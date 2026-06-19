use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akraz_identity::{FileIdentityStore, PairingIdentityDocument, TrustedPeerIdentity};
use akraz_ipc::{
    DaemonCrashMarker, DaemonLogEntry, DaemonLogsTail, DaemonLogsTailParams, DaemonShutdownParams,
    DaemonShutdownResult, DaemonStatus, DaemonStatusParams, DiagnosticsKeyboardLayout,
    DiagnosticsKeyboardLayoutParams, DiagnosticsScreenTopology, DiagnosticsScreenTopologyParams,
    DiagnosticsSnapshot, DiagnosticsSupportBundle, InputReleaseAllParams, InputReleaseAllResult,
    IpcCallError, IpcEndpoint, IpcTransportError, JSONRPC_VERSION, JsonRpcFailure, JsonRpcRequest,
    JsonRpcSuccess, LocalIpcClient, METHOD_DAEMON_LOGS_TAIL, METHOD_DAEMON_SHUTDOWN,
    METHOD_DAEMON_STATUS, METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT, METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
    METHOD_INPUT_RELEASE_ALL, METHOD_PERMISSIONS_PROBE, METHOD_SESSION_CONNECT,
    METHOD_SESSION_DISCONNECT, OsLocalIpcClient, PermissionsProbe, PermissionsProbeParams,
    SessionConnectResult, SessionDisconnectParams, SessionDisconnectResult,
    build_diagnostics_latency_histogram, build_diagnostics_snapshot,
    build_diagnostics_support_bundle_with_previous_crash, call_json_rpc,
    resolve_current_default_endpoint,
};
use akraz_protocol::CapabilityFlags;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::async_runtime::Receiver;
use tauri_plugin_shell::{
    ShellExt,
    process::{CommandChild, CommandEvent},
};

const LOCAL_REQUEST_ID: &str = "tauri";
const DAEMON_PATH_ENV: &str = "AKRAZ_DAEMON_PATH";
const DAEMON_CAPTURE_INPUT_ENV: &str = "AKRAZ_DAEMON_CAPTURE_INPUT";
const DAEMON_SIDECAR_NAME: &str = "akraz-daemon";
const DAEMON_SERVE_ARG: &str = "--serve";
const DAEMON_CAPTURE_INPUT_ARG: &str = "--capture-input";
const DAEMON_EDGE_BINDING_ARG: &str = "--edge-binding";
const DAEMON_PEER_LISTEN_ARG: &str = "--peer-listen";
const DAEMON_IDENTITY_STORE_ARG: &str = "--identity-store";
const DAEMON_IDENTITY_DISPLAY_NAME_ARG: &str = "--identity-display-name";
const DAEMON_CRASH_MARKER_ARG: &str = "--crash-marker";
const DAEMON_IDENTITY_DISPLAY_NAME: &str = "Akraz Desktop";
const DAEMON_LIFECYCLE_SMOKE_FLAG: &str = "--akraz-smoke-daemon-lifecycle";
const DAEMON_SETTINGS_START_SMOKE_FLAG: &str = "--akraz-smoke-settings-start";
const SETTINGS_FILE_NAME: &str = "settings.json";
const IDENTITY_STORE_DIR_NAME: &str = "secrets";
const IDENTITY_STORE_FILE_NAME: &str = "identity.json";
const DAEMON_CRASH_DIR_NAME: &str = "crash";
const DAEMON_CRASH_MARKER_FILE_NAME: &str = "daemon-crash.json";
const DAEMON_START_RETRIES: usize = 50;
const DAEMON_START_RETRY_DELAY: Duration = Duration::from_millis(40);
const DAEMON_STOP_RETRIES: usize = 50;
const DAEMON_STOP_RETRY_DELAY: Duration = Duration::from_millis(40);

type ManagedDaemon = Arc<Mutex<DaemonProcessState>>;

pub fn run() -> Result<(), String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if has_exact_arg(&args, OsStr::new(DAEMON_SETTINGS_START_SMOKE_FLAG)) {
        return run_daemon_settings_start_smoke();
    }
    if has_exact_arg(&args, OsStr::new(DAEMON_LIFECYCLE_SMOKE_FLAG)) {
        return run_daemon_lifecycle_smoke();
    }

    run_gui().map_err(|error| error.to_string())
}

fn run_gui() -> tauri::Result<()> {
    app_builder(Arc::new(Mutex::new(DaemonProcessState::default()))).run(tauri::generate_context!())
}

fn app_builder(managed: ManagedDaemon) -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(managed)
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            permissions_probe,
            screen_topology_probe,
            diagnostics_snapshot,
            diagnostics_support_bundle,
            daemon_start,
            daemon_stop,
            session_connect,
            session_disconnect,
            input_release_all,
            identity_show,
            identity_list_trusted,
            identity_trust,
            identity_forget_trusted,
            layout_get,
            layout_set,
            settings_load,
            settings_save
        ])
}

fn has_exact_arg<I>(args: I, expected: &OsStr) -> bool
where
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    args.into_iter()
        .any(|argument| argument.as_ref() == expected)
}

fn run_daemon_lifecycle_smoke() -> Result<(), String> {
    let managed = Arc::new(Mutex::new(DaemonProcessState::default()));
    let app = app_builder(Arc::clone(&managed))
        .build(tauri::generate_context!())
        .map_err(|error| format!("failed to initialize Akraz smoke app: {error}"))?;
    let handle = app.handle().clone();
    let initial = refresh_daemon_snapshot(&managed)?;
    let mut report = DaemonLifecycleSmokeReport::new(initial.clone());

    if initial.phase != DaemonLifecyclePhase::NotRunning {
        print_smoke_report(&report)?;
        return Err(format!(
            "daemon lifecycle smoke requires no existing daemon, but initial phase was {:?}",
            initial.phase
        ));
    }

    let started = start_daemon(&handle, &managed, DaemonStartOptions::default())?;
    report.started = Some(started.clone());
    if started.phase != DaemonLifecyclePhase::Running {
        report.record_stop(stop_daemon(&managed)?);
        print_smoke_report(&report)?;
        return Err(format!(
            "daemon lifecycle smoke expected running after start, got {:?}",
            started.phase
        ));
    }
    if let Err(error) = record_smoke_permissions(&managed, &mut report) {
        print_smoke_report(&report)?;
        return Err(error);
    }

    let stopped = stop_daemon(&managed)?;
    let stopped_phase = stopped.snapshot.phase;
    report.record_stop(stopped);
    print_smoke_report(&report)?;
    if stopped_phase == DaemonLifecyclePhase::Running {
        return Err(
            "daemon lifecycle smoke expected daemon to stop, but it is still running.".to_string(),
        );
    }

    Ok(())
}

fn run_daemon_settings_start_smoke() -> Result<(), String> {
    let managed = Arc::new(Mutex::new(DaemonProcessState::default()));
    let app = app_builder(Arc::clone(&managed))
        .build(tauri::generate_context!())
        .map_err(|error| format!("failed to initialize Akraz settings smoke app: {error}"))?;
    let handle = app.handle().clone();
    let initial = refresh_daemon_snapshot(&managed)?;
    let mut report = DaemonLifecycleSmokeReport::new(initial.clone());

    if initial.phase != DaemonLifecyclePhase::NotRunning {
        print_smoke_report(&report)?;
        return Err(format!(
            "daemon settings smoke requires no existing daemon, but initial phase was {:?}",
            initial.phase
        ));
    }

    let settings_path = temp_settings_smoke_path();
    let settings = settings_start_smoke_settings();
    let saved_settings = save_settings_to_path(&settings_path, settings)?;
    let loaded_settings = load_settings_from_path(&settings_path)?;
    let _ = fs::remove_file(&settings_path);
    if loaded_settings != saved_settings {
        report.settings = Some(loaded_settings);
        print_smoke_report(&report)?;
        return Err(
            "daemon settings smoke loaded settings did not match saved settings.".to_string(),
        );
    }
    report.settings = Some(loaded_settings.clone());

    let started = start_daemon(&handle, &managed, DaemonStartOptions::from(loaded_settings))?;
    report.started = Some(started.clone());
    if started.phase != DaemonLifecyclePhase::Running {
        report.record_stop(stop_daemon(&managed)?);
        print_smoke_report(&report)?;
        return Err(format!(
            "daemon settings smoke expected running after configured start, got {:?}",
            started.phase
        ));
    }
    if let Err(error) = record_smoke_permissions(&managed, &mut report) {
        print_smoke_report(&report)?;
        return Err(error);
    }

    let stopped = stop_daemon(&managed)?;
    let stopped_phase = stopped.snapshot.phase;
    report.record_stop(stopped);
    print_smoke_report(&report)?;
    if stopped_phase == DaemonLifecyclePhase::Running {
        return Err(
            "daemon settings smoke expected daemon to stop, but it is still running.".to_string(),
        );
    }

    Ok(())
}

fn settings_start_smoke_settings() -> AppSettings {
    AppSettings {
        capture_input: true,
        peer_listen_address: "127.0.0.1:0".to_string(),
        edge_bindings: vec![DaemonEdgeBindingOption {
            local_edge: DaemonScreenEdgeOption::Right,
            peer_id: "linux-laptop".to_string(),
            remote_edge: DaemonScreenEdgeOption::Left,
        }],
        manual_peer_addresses: vec![ManualPeerAddressSetting {
            peer_id: "linux-laptop".to_string(),
            address: "127.0.0.1:4455".to_string(),
        }],
    }
}

fn record_smoke_permissions(
    managed: &ManagedDaemon,
    report: &mut DaemonLifecycleSmokeReport,
) -> Result<(), String> {
    match call_daemon_permissions_probe() {
        Ok(permissions) => {
            report.permissions = Some(permissions);
            Ok(())
        }
        Err(error) => {
            report.record_stop(stop_daemon(managed)?);
            Err(format!("daemon smoke permissions probe failed: {error}"))
        }
    }
}

fn temp_settings_smoke_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "akraz-settings-start-smoke-{}-{}.json",
        std::process::id(),
        monotonic_smoke_suffix()
    ))
}

fn monotonic_smoke_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn print_smoke_report(report: &DaemonLifecycleSmokeReport) -> Result<(), String> {
    let line = serde_json::to_string(report)
        .map_err(|error| format!("failed to encode daemon lifecycle smoke report: {error}"))?;
    println!("{line}");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonLifecycleSmokeReport {
    initial: DaemonLifecycleSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<AppSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permissions: Option<PermissionsProbe>,
    started: Option<DaemonLifecycleSnapshot>,
    stopped: Option<DaemonLifecycleSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_method: Option<DaemonStopMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shutdown: Option<DaemonShutdownResult>,
}

impl DaemonLifecycleSmokeReport {
    fn new(initial: DaemonLifecycleSnapshot) -> Self {
        Self {
            initial,
            settings: None,
            permissions: None,
            started: None,
            stopped: None,
            stop_method: None,
            shutdown: None,
        }
    }

    fn record_stop(&mut self, outcome: DaemonStopOutcome) {
        self.stop_method = Some(outcome.method);
        self.shutdown = outcome.shutdown;
        self.stopped = Some(outcome.snapshot);
    }
}

#[tauri::command]
async fn daemon_status(
    app: tauri::AppHandle,
    managed: tauri::State<'_, ManagedDaemon>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    tauri::async_runtime::spawn_blocking(move || {
        refresh_daemon_snapshot(&managed)
            .map(|snapshot| attach_previous_daemon_crash(&app, snapshot))
    })
    .await
    .map_err(|error| format!("daemon status task failed: {error}"))?
}

#[tauri::command]
async fn permissions_probe() -> Result<PermissionsProbe, String> {
    tauri::async_runtime::spawn_blocking(call_daemon_permissions_probe)
        .await
        .map_err(|error| format!("permissions probe task failed: {error}"))?
}

#[tauri::command]
async fn screen_topology_probe() -> Result<DiagnosticsScreenTopology, String> {
    tauri::async_runtime::spawn_blocking(call_daemon_screen_topology)
        .await
        .map_err(|error| format!("screen topology task failed: {error}"))?
}

#[tauri::command]
async fn diagnostics_snapshot() -> Result<DiagnosticsSnapshot, String> {
    tauri::async_runtime::spawn_blocking(call_daemon_diagnostics_snapshot)
        .await
        .map_err(|error| format!("diagnostics snapshot task failed: {error}"))?
}

#[tauri::command]
async fn diagnostics_support_bundle(
    app: tauri::AppHandle,
) -> Result<DiagnosticsSupportBundle, String> {
    tauri::async_runtime::spawn_blocking(move || call_daemon_diagnostics_support_bundle(&app))
        .await
        .map_err(|error| format!("diagnostics support bundle task failed: {error}"))?
}

#[tauri::command]
async fn daemon_start(
    app: tauri::AppHandle,
    managed: tauri::State<'_, ManagedDaemon>,
    options: Option<DaemonStartOptions>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    let options = options.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        start_daemon(&app, &managed, options)
            .map(|snapshot| attach_previous_daemon_crash(&app, snapshot))
    })
    .await
    .map_err(|error| format!("daemon start task failed: {error}"))?
}

#[tauri::command]
async fn daemon_stop(
    app: tauri::AppHandle,
    managed: tauri::State<'_, ManagedDaemon>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    tauri::async_runtime::spawn_blocking(move || {
        stop_daemon(&managed).map(|outcome| attach_previous_daemon_crash(&app, outcome.snapshot))
    })
    .await
    .map_err(|error| format!("daemon stop task failed: {error}"))?
}

#[tauri::command]
async fn session_connect(params: SessionConnectOptions) -> Result<DaemonLifecycleSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || connect_daemon_session(params))
        .await
        .map_err(|error| format!("session connect task failed: {error}"))?
}

#[tauri::command]
async fn session_disconnect() -> Result<DaemonLifecycleSnapshot, String> {
    tauri::async_runtime::spawn_blocking(disconnect_daemon_session)
        .await
        .map_err(|error| format!("session disconnect task failed: {error}"))?
}

#[tauri::command]
async fn input_release_all() -> Result<DaemonLifecycleSnapshot, String> {
    tauri::async_runtime::spawn_blocking(release_all_daemon_inputs)
        .await
        .map_err(|error| format!("input release task failed: {error}"))?
}

#[tauri::command]
async fn identity_show(app: tauri::AppHandle) -> Result<IdentityShowResult, String> {
    tauri::async_runtime::spawn_blocking(move || load_identity_document(&app))
        .await
        .map_err(|error| format!("identity show task failed: {error}"))?
}

#[tauri::command]
async fn identity_list_trusted(
    app: tauri::AppHandle,
) -> Result<IdentityTrustedPeersResult, String> {
    tauri::async_runtime::spawn_blocking(move || list_trusted_identities(&app))
        .await
        .map_err(|error| format!("identity trusted peers task failed: {error}"))?
}

#[tauri::command]
async fn identity_trust(
    app: tauri::AppHandle,
    params: IdentityTrustOptions,
) -> Result<IdentityTrustResult, String> {
    tauri::async_runtime::spawn_blocking(move || trust_identity_document(&app, params))
        .await
        .map_err(|error| format!("identity trust task failed: {error}"))?
}

#[tauri::command]
async fn identity_forget_trusted(
    app: tauri::AppHandle,
    params: IdentityForgetTrustedOptions,
) -> Result<IdentityForgetTrustedResult, String> {
    tauri::async_runtime::spawn_blocking(move || forget_trusted_identity(&app, params))
        .await
        .map_err(|error| format!("identity forget trusted task failed: {error}"))?
}

#[tauri::command]
async fn layout_get(app: tauri::AppHandle) -> Result<LayoutSettings, String> {
    tauri::async_runtime::spawn_blocking(move || load_app_layout(&app))
        .await
        .map_err(|error| format!("layout get task failed: {error}"))?
}

#[tauri::command]
async fn layout_set(
    app: tauri::AppHandle,
    layout: LayoutSettings,
) -> Result<LayoutSettings, String> {
    tauri::async_runtime::spawn_blocking(move || save_app_layout(&app, layout))
        .await
        .map_err(|error| format!("layout set task failed: {error}"))?
}

#[tauri::command]
async fn settings_load(app: tauri::AppHandle) -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || load_app_settings(&app))
        .await
        .map_err(|error| format!("settings load task failed: {error}"))?
}

#[tauri::command]
async fn settings_save(
    app: tauri::AppHandle,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || save_app_settings(&app, settings))
        .await
        .map_err(|error| format!("settings save task failed: {error}"))?
}

#[derive(Debug, Default)]
struct DaemonProcessState {
    child: Option<ManagedDaemonChild>,
}

#[derive(Debug)]
enum ManagedDaemonChild {
    Os(Child),
    Sidecar(CommandChild),
}

impl ManagedDaemonChild {
    fn pid(&self) -> u32 {
        match self {
            Self::Os(child) => child.id(),
            Self::Sidecar(child) => child.pid(),
        }
    }

    fn is_running(&mut self) -> bool {
        match self {
            Self::Os(child) => matches!(child.try_wait(), Ok(None)),
            Self::Sidecar(_) => true,
        }
    }

    fn kill(self) -> Result<(), String> {
        match self {
            Self::Os(mut child) => {
                child
                    .kill()
                    .map_err(|error| format!("failed to stop akraz-daemon: {error}"))?;
                child.wait().map_err(|error| {
                    format!("akraz-daemon stopped, but process cleanup failed: {error}")
                })?;
                Ok(())
            }
            Self::Sidecar(child) => child
                .kill()
                .map_err(|error| format!("failed to stop akraz-daemon sidecar: {error}")),
        }
    }
}

#[derive(Debug)]
struct SpawnedDaemonProcess {
    child: ManagedDaemonChild,
    sidecar_events: Option<Receiver<CommandEvent>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppSettings {
    #[serde(default)]
    capture_input: bool,
    #[serde(default)]
    peer_listen_address: String,
    #[serde(default)]
    edge_bindings: Vec<DaemonEdgeBindingOption>,
    #[serde(default)]
    manual_peer_addresses: Vec<ManualPeerAddressSetting>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayoutSettings {
    #[serde(default)]
    edge_bindings: Vec<DaemonEdgeBindingOption>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DaemonStartOptions {
    capture_input: Option<bool>,
    peer_listen_address: Option<String>,
    #[serde(default)]
    edge_bindings: Vec<DaemonEdgeBindingOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionConnectOptions {
    peer_id: String,
    local_device_id: String,
    address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityShowResult {
    device_id: String,
    display_name: String,
    fingerprint: String,
    capabilities: CapabilityFlags,
    document: PairingIdentityDocument,
    document_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityTrustedPeer {
    peer_id: String,
    display_name: String,
    fingerprint: String,
    capabilities: CapabilityFlags,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityTrustedPeersResult {
    peers: Vec<IdentityTrustedPeer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdentityTrustOptions {
    peer_document_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityTrustResult {
    trusted: bool,
    peer_id: String,
    display_name: String,
    fingerprint: String,
    capabilities: CapabilityFlags,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdentityForgetTrustedOptions {
    peer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityForgetTrustedResult {
    forgotten: bool,
    peer_id: String,
}

impl From<AppSettings> for DaemonStartOptions {
    fn from(settings: AppSettings) -> Self {
        Self {
            capture_input: Some(settings.capture_input),
            peer_listen_address: non_empty_string(settings.peer_listen_address),
            edge_bindings: settings.edge_bindings,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DaemonEdgeBindingOption {
    local_edge: DaemonScreenEdgeOption,
    peer_id: String,
    remote_edge: DaemonScreenEdgeOption,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManualPeerAddressSetting {
    peer_id: String,
    address: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DaemonScreenEdgeOption {
    Left,
    Right,
    Top,
    Bottom,
}

impl DaemonScreenEdgeOption {
    fn daemon_arg(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DaemonLifecyclePhase {
    NotRunning,
    Starting,
    Running,
    Unreachable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonLifecycleSnapshot {
    phase: DaemonLifecyclePhase,
    status: Option<DaemonStatus>,
    detail: Option<String>,
    managed_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_crash: Option<Box<DaemonCrashMarker>>,
}

impl DaemonLifecycleSnapshot {
    fn running(status: DaemonStatus, managed_pid: Option<u32>) -> Self {
        Self {
            phase: DaemonLifecyclePhase::Running,
            status: Some(status),
            detail: None,
            managed_pid,
            previous_crash: None,
        }
    }

    fn running_without_managed_pid(status: DaemonStatus) -> Self {
        Self::running(status, None)
    }

    fn not_running(detail: impl Into<String>) -> Self {
        Self::without_status(DaemonLifecyclePhase::NotRunning, detail)
    }

    fn starting(detail: impl Into<String>, managed_pid: Option<u32>) -> Self {
        Self {
            phase: DaemonLifecyclePhase::Starting,
            status: None,
            detail: Some(detail.into()),
            managed_pid,
            previous_crash: None,
        }
    }

    fn unreachable(detail: impl Into<String>) -> Self {
        Self::without_status(DaemonLifecyclePhase::Unreachable, detail)
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self::without_status(DaemonLifecyclePhase::Failed, detail)
    }

    fn without_status(phase: DaemonLifecyclePhase, detail: impl Into<String>) -> Self {
        Self {
            phase,
            status: None,
            detail: Some(detail.into()),
            managed_pid: None,
            previous_crash: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn with_managed_pid(mut self, managed_pid: Option<u32>) -> Self {
        self.managed_pid = managed_pid;
        self
    }

    fn with_previous_crash(mut self, previous_crash: Option<DaemonCrashMarker>) -> Self {
        self.previous_crash = previous_crash.map(Box::new);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DaemonStopMethod {
    GracefulShutdown,
    ForcedKill,
    Unmanaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonStopOutcome {
    snapshot: DaemonLifecycleSnapshot,
    method: DaemonStopMethod,
    shutdown: Option<DaemonShutdownResult>,
}

impl DaemonStopOutcome {
    fn new(
        snapshot: DaemonLifecycleSnapshot,
        method: DaemonStopMethod,
        shutdown: Option<DaemonShutdownResult>,
    ) -> Self {
        Self {
            snapshot,
            method,
            shutdown,
        }
    }

    fn graceful(snapshot: DaemonLifecycleSnapshot, shutdown: DaemonShutdownResult) -> Self {
        Self::new(snapshot, DaemonStopMethod::GracefulShutdown, Some(shutdown))
    }

    fn forced(snapshot: DaemonLifecycleSnapshot, shutdown: Option<DaemonShutdownResult>) -> Self {
        Self::new(snapshot, DaemonStopMethod::ForcedKill, shutdown)
    }

    fn unmanaged(snapshot: DaemonLifecycleSnapshot) -> Self {
        Self::new(snapshot, DaemonStopMethod::Unmanaged, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonCallFailure {
    NotRunning(String),
    Unreachable(String),
    Failed(String),
}

impl DaemonCallFailure {
    fn into_snapshot(self) -> DaemonLifecycleSnapshot {
        match self {
            Self::NotRunning(detail) => DaemonLifecycleSnapshot::not_running(detail),
            Self::Unreachable(detail) => DaemonLifecycleSnapshot::unreachable(detail),
            Self::Failed(detail) => DaemonLifecycleSnapshot::failed(detail),
        }
    }

    fn to_user_message(&self) -> String {
        match self {
            Self::NotRunning(detail) | Self::Unreachable(detail) | Self::Failed(detail) => {
                detail.clone()
            }
        }
    }
}

fn refresh_daemon_snapshot(managed: &ManagedDaemon) -> Result<DaemonLifecycleSnapshot, String> {
    let managed_pid = managed_daemon_pid(managed)?;
    Ok(read_daemon_snapshot().with_managed_pid(managed_pid))
}

fn start_daemon(
    app: &tauri::AppHandle,
    managed: &ManagedDaemon,
    options: DaemonStartOptions,
) -> Result<DaemonLifecycleSnapshot, String> {
    let current = refresh_daemon_snapshot(managed)?;
    match current.phase {
        DaemonLifecyclePhase::Running => {
            return Ok(current.with_detail("Akraz is already running."));
        }
        DaemonLifecyclePhase::NotRunning => {}
        DaemonLifecyclePhase::Starting => return Ok(current),
        DaemonLifecyclePhase::Unreachable | DaemonLifecyclePhase::Failed => {
            return Ok(current);
        }
    }
    if let Some(pid) = managed_daemon_pid(managed)? {
        return Ok(DaemonLifecycleSnapshot::starting(
            "Akraz is already starting.",
            Some(pid),
        ));
    }

    let spawned = match spawn_daemon_process(app, &options) {
        Ok(spawned) => spawned,
        Err(error) => return Ok(DaemonLifecycleSnapshot::failed(error)),
    };
    let pid = store_managed_child(managed, spawned.child)?;
    if let Some(events) = spawned.sidecar_events {
        watch_sidecar_termination(Arc::clone(managed), pid, events);
    }

    for _ in 0..DAEMON_START_RETRIES {
        if managed_daemon_pid(managed)?.is_none() {
            return Ok(DaemonLifecycleSnapshot::failed(
                "akraz-daemon exited before it became reachable.",
            ));
        }

        let snapshot = read_daemon_snapshot().with_managed_pid(Some(pid));
        if snapshot.phase == DaemonLifecyclePhase::Running {
            return Ok(snapshot.with_detail("Akraz is running."));
        }

        thread::sleep(DAEMON_START_RETRY_DELAY);
    }

    Ok(DaemonLifecycleSnapshot::starting(
        "Akraz is starting, but it has not answered yet.",
        Some(pid),
    ))
}

fn stop_daemon(managed: &ManagedDaemon) -> Result<DaemonStopOutcome, String> {
    if managed_daemon_pid(managed)?.is_none() {
        return Ok(DaemonStopOutcome::unmanaged(
            read_daemon_snapshot()
                .with_detail("This app did not start the current Akraz background process."),
        ));
    }

    let shutdown = call_daemon_shutdown().ok();
    if let Some(shutdown_result) = &shutdown {
        let snapshot = wait_for_stopped_daemon_snapshot();
        if snapshot.phase == DaemonLifecyclePhase::NotRunning {
            clear_managed_child(managed)?;
            return Ok(DaemonStopOutcome::graceful(
                snapshot.with_detail("Akraz stopped."),
                shutdown_result.clone(),
            ));
        }
    }

    let Some(child) = take_managed_child(managed)? else {
        let method = if shutdown.is_some() {
            DaemonStopMethod::GracefulShutdown
        } else {
            DaemonStopMethod::Unmanaged
        };
        return Ok(DaemonStopOutcome::new(
            read_daemon_snapshot().with_detail("Akraz stopped."),
            method,
            shutdown,
        ));
    };

    match child.kill() {
        Ok(()) => Ok(DaemonStopOutcome::forced(
            wait_for_stopped_daemon_snapshot().with_detail(
                "Akraz graceful stop did not settle, so the managed process was stopped.",
            ),
            shutdown,
        )),
        Err(error) => Ok(DaemonStopOutcome::forced(
            DaemonLifecycleSnapshot::failed(error),
            shutdown,
        )),
    }
}

fn read_daemon_snapshot() -> DaemonLifecycleSnapshot {
    match call_daemon_status() {
        Ok(status) => DaemonLifecycleSnapshot::running(status, None),
        Err(snapshot) => snapshot,
    }
}

fn attach_previous_daemon_crash(
    app: &tauri::AppHandle,
    snapshot: DaemonLifecycleSnapshot,
) -> DaemonLifecycleSnapshot {
    snapshot.with_previous_crash(read_previous_daemon_crash(app))
}

fn read_previous_daemon_crash(app: &tauri::AppHandle) -> Option<DaemonCrashMarker> {
    daemon_crash_marker_path(app)
        .ok()
        .and_then(|path| read_previous_daemon_crash_from_path(&path).ok().flatten())
}

fn read_previous_daemon_crash_from_path(path: &Path) -> Result<Option<DaemonCrashMarker>, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read previous daemon crash marker {}: {error}",
                path.display()
            ));
        }
    };

    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("failed to parse previous daemon crash marker: {error}"))
}

fn wait_for_stopped_daemon_snapshot() -> DaemonLifecycleSnapshot {
    let mut last_snapshot = read_daemon_snapshot();
    for _ in 0..DAEMON_STOP_RETRIES {
        if last_snapshot.phase == DaemonLifecyclePhase::NotRunning {
            return last_snapshot.with_detail("Akraz stopped.");
        }

        thread::sleep(DAEMON_STOP_RETRY_DELAY);
        last_snapshot = read_daemon_snapshot();
    }

    last_snapshot.with_detail(
        "Akraz stop was requested, but the daemon endpoint did not settle before the timeout.",
    )
}

fn call_daemon_status() -> Result<DaemonStatus, DaemonLifecycleSnapshot> {
    call_daemon_status_result().map_err(|error| error.into_snapshot())
}

fn call_daemon_status_result() -> Result<DaemonStatus, DaemonCallFailure> {
    let client = build_default_daemon_client()?;
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()))?;

    parse_json_rpc_response::<DaemonStatus>(&response_line, "status")
        .map_err(DaemonCallFailure::Failed)
}

fn call_daemon_shutdown() -> Result<DaemonShutdownResult, String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_SHUTDOWN,
        DaemonShutdownParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()).to_user_message())?;

    parse_json_rpc_response::<DaemonShutdownResult>(&response_line, "daemon shutdown")
}

fn call_daemon_permissions_probe() -> Result<PermissionsProbe, String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_PERMISSIONS_PROBE,
        PermissionsProbeParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()).to_user_message())?;

    parse_json_rpc_response::<PermissionsProbe>(&response_line, "permissions probe")
}

fn call_daemon_screen_topology() -> Result<DiagnosticsScreenTopology, String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    call_daemon_json_rpc::<DiagnosticsScreenTopology, _>(
        &client,
        &JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
            DiagnosticsScreenTopologyParams::default(),
        ),
        "screen topology",
    )
}

fn call_daemon_diagnostics_snapshot() -> Result<DiagnosticsSnapshot, String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    let mut latency_samples = Vec::new();
    let (status, status_latency) = call_daemon_json_rpc_timed::<DaemonStatus, _>(
        &client,
        &JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DAEMON_STATUS,
            DaemonStatusParams::default(),
        ),
        "status",
    )?;
    latency_samples.push(status_latency);
    let (permissions, permissions_latency) = call_daemon_json_rpc_timed::<PermissionsProbe, _>(
        &client,
        &JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_PERMISSIONS_PROBE,
            PermissionsProbeParams::default(),
        ),
        "permissions probe",
    )?;
    latency_samples.push(permissions_latency);
    let screen_topology = call_daemon_json_rpc_timed::<DiagnosticsScreenTopology, _>(
        &client,
        &JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
            DiagnosticsScreenTopologyParams::default(),
        ),
        "screen topology",
    )
    .map(|(topology, latency)| {
        latency_samples.push(latency);
        topology
    })
    .ok();
    let keyboard_layout = call_daemon_json_rpc_timed::<DiagnosticsKeyboardLayout, _>(
        &client,
        &JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT,
            DiagnosticsKeyboardLayoutParams::default(),
        ),
        "keyboard layout",
    )
    .map(|(keyboard_layout, latency)| {
        latency_samples.push(latency);
        keyboard_layout
    })
    .ok();

    Ok(build_diagnostics_snapshot(
        status,
        permissions,
        screen_topology,
        keyboard_layout,
        build_diagnostics_latency_histogram(&latency_samples),
        "akraz-app",
        env!("CARGO_PKG_VERSION"),
    ))
}

fn call_daemon_diagnostics_support_bundle(
    app: &tauri::AppHandle,
) -> Result<DiagnosticsSupportBundle, String> {
    let snapshot = call_daemon_diagnostics_snapshot()?;
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;

    Ok(build_diagnostics_support_bundle_with_previous_crash(
        snapshot,
        collect_recent_daemon_logs(&client),
        read_previous_daemon_crash(app),
        "akraz-app",
        env!("CARGO_PKG_VERSION"),
    ))
}

fn collect_recent_daemon_logs(client: &OsLocalIpcClient) -> Vec<DaemonLogEntry> {
    call_daemon_json_rpc::<DaemonLogsTail, _>(
        client,
        &JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DAEMON_LOGS_TAIL,
            DaemonLogsTailParams::default(),
        ),
        "daemon logs tail",
    )
    .map(|tail| tail.entries)
    .unwrap_or_default()
}

fn call_daemon_json_rpc<T, P>(
    client: &OsLocalIpcClient,
    request: &JsonRpcRequest<P>,
    label: &str,
) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
    P: Serialize,
{
    let response_line = call_json_rpc(client, request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()).to_user_message())?;

    parse_json_rpc_response::<T>(&response_line, label)
}

fn call_daemon_json_rpc_timed<T, P>(
    client: &OsLocalIpcClient,
    request: &JsonRpcRequest<P>,
    label: &str,
) -> Result<(T, u128), String>
where
    T: for<'de> Deserialize<'de>,
    P: Serialize,
{
    let started = Instant::now();
    let result = call_daemon_json_rpc(client, request, label)?;
    Ok((result, started.elapsed().as_micros()))
}

fn connect_daemon_session(
    params: SessionConnectOptions,
) -> Result<DaemonLifecycleSnapshot, String> {
    let params = normalize_session_connect_options(params)?;
    call_daemon_session_connect(params)?;
    call_daemon_status_result()
        .map(DaemonLifecycleSnapshot::running_without_managed_pid)
        .map_err(|error| error.to_user_message())
}

fn call_daemon_session_connect(params: SessionConnectOptions) -> Result<(), String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    let request = JsonRpcRequest::new(LOCAL_REQUEST_ID, METHOD_SESSION_CONNECT, params);
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()).to_user_message())?;

    parse_json_rpc_response::<SessionConnectResult>(&response_line, "session connect").map(|_| ())
}

fn disconnect_daemon_session() -> Result<DaemonLifecycleSnapshot, String> {
    call_daemon_session_disconnect()?;
    call_daemon_status_result()
        .map(DaemonLifecycleSnapshot::running_without_managed_pid)
        .map_err(|error| error.to_user_message())
}

fn call_daemon_session_disconnect() -> Result<(), String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_SESSION_DISCONNECT,
        SessionDisconnectParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()).to_user_message())?;

    parse_json_rpc_response::<SessionDisconnectResult>(&response_line, "session disconnect")
        .map(|_| ())
}

fn release_all_daemon_inputs() -> Result<DaemonLifecycleSnapshot, String> {
    call_daemon_input_release_all()?;
    call_daemon_status_result()
        .map(DaemonLifecycleSnapshot::running_without_managed_pid)
        .map_err(|error| error.to_user_message())
}

fn call_daemon_input_release_all() -> Result<(), String> {
    let client = build_default_daemon_client().map_err(|error| error.to_user_message())?;
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_INPUT_RELEASE_ALL,
        InputReleaseAllParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()).to_user_message())?;

    parse_json_rpc_response::<InputReleaseAllResult>(&response_line, "input release all")
        .map(|_| ())
}

fn build_default_daemon_client() -> Result<OsLocalIpcClient, DaemonCallFailure> {
    let endpoint = resolve_current_default_endpoint()
        .map_err(|error| DaemonCallFailure::Failed(error.to_string()))?;

    Ok(OsLocalIpcClient::new(endpoint))
}

fn load_app_settings(app: &tauri::AppHandle) -> Result<AppSettings, String> {
    let path = app_settings_path(app)?;
    load_settings_from_path(&path)
}

fn save_app_settings(app: &tauri::AppHandle, settings: AppSettings) -> Result<AppSettings, String> {
    let path = app_settings_path(app)?;
    save_settings_to_path(&path, settings)
}

fn load_app_layout(app: &tauri::AppHandle) -> Result<LayoutSettings, String> {
    let path = app_settings_path(app)?;
    load_layout_from_path(&path)
}

fn save_app_layout(
    app: &tauri::AppHandle,
    layout: LayoutSettings,
) -> Result<LayoutSettings, String> {
    let path = app_settings_path(app)?;
    save_layout_to_path(&path, layout)
}

fn load_identity_document(app: &tauri::AppHandle) -> Result<IdentityShowResult, String> {
    let path = daemon_identity_store_path(app)?;
    build_identity_show_result_from_path(&path, DAEMON_IDENTITY_DISPLAY_NAME)
}

fn list_trusted_identities(app: &tauri::AppHandle) -> Result<IdentityTrustedPeersResult, String> {
    let path = daemon_identity_store_path(app)?;
    list_trusted_identities_from_path(&path, DAEMON_IDENTITY_DISPLAY_NAME)
}

fn trust_identity_document(
    app: &tauri::AppHandle,
    params: IdentityTrustOptions,
) -> Result<IdentityTrustResult, String> {
    let path = daemon_identity_store_path(app)?;
    trust_identity_document_from_json(
        &path,
        DAEMON_IDENTITY_DISPLAY_NAME,
        &params.peer_document_json,
    )
}

fn forget_trusted_identity(
    app: &tauri::AppHandle,
    params: IdentityForgetTrustedOptions,
) -> Result<IdentityForgetTrustedResult, String> {
    let path = daemon_identity_store_path(app)?;
    forget_trusted_identity_from_path(&path, DAEMON_IDENTITY_DISPLAY_NAME, &params.peer_id)
}

fn app_settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("failed to resolve Akraz settings directory: {error}"))?;

    Ok(directory.join(SETTINGS_FILE_NAME))
}

fn daemon_identity_store_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("failed to resolve Akraz identity directory: {error}"))?;

    Ok(identity_store_path_from_config_dir(directory))
}

fn daemon_crash_marker_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("failed to resolve Akraz crash marker directory: {error}"))?;

    Ok(daemon_crash_marker_path_from_config_dir(directory))
}

fn identity_store_path_from_config_dir(config_dir: PathBuf) -> PathBuf {
    config_dir
        .join(IDENTITY_STORE_DIR_NAME)
        .join(IDENTITY_STORE_FILE_NAME)
}

fn daemon_crash_marker_path_from_config_dir(config_dir: PathBuf) -> PathBuf {
    config_dir
        .join(DAEMON_CRASH_DIR_NAME)
        .join(DAEMON_CRASH_MARKER_FILE_NAME)
}

fn build_identity_show_result_from_path(
    path: &Path,
    display_name: &str,
) -> Result<IdentityShowResult, String> {
    let store = FileIdentityStore::new(path);
    let identity = store.load_or_create(display_name).map_err(|source| {
        format_identity_store_error("failed to load Akraz identity", path, &source)
    })?;
    let document = PairingIdentityDocument::from_device_identity(
        identity.identity(),
        default_pairing_capabilities(),
    );
    let document_json = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("failed to encode Akraz identity: {error}"))?;

    Ok(IdentityShowResult {
        device_id: document.device_id().to_string(),
        display_name: document.display_name().to_string(),
        fingerprint: document.fingerprint().to_string(),
        capabilities: document.capabilities(),
        document,
        document_json,
    })
}

fn list_trusted_identities_from_path(
    path: &Path,
    display_name: &str,
) -> Result<IdentityTrustedPeersResult, String> {
    let store = FileIdentityStore::new(path);
    store.load_or_create(display_name).map_err(|source| {
        format_identity_store_error("failed to load Akraz identity", path, &source)
    })?;
    let peers = store.list_trusted_peers().map_err(|source| {
        format_identity_store_error("failed to list trusted peers", path, &source)
    })?;

    Ok(IdentityTrustedPeersResult {
        peers: peers.into_iter().map(IdentityTrustedPeer::from).collect(),
    })
}

fn trust_identity_document_from_json(
    path: &Path,
    display_name: &str,
    peer_document_json: &str,
) -> Result<IdentityTrustResult, String> {
    let peer_document_json = peer_document_json.trim();
    if peer_document_json.is_empty() {
        return Err("peer identity is required.".to_string());
    }

    let store = FileIdentityStore::new(path);
    store.load_or_create(display_name).map_err(|source| {
        format_identity_store_error("failed to load Akraz identity", path, &source)
    })?;
    let document: PairingIdentityDocument = serde_json::from_str(peer_document_json)
        .map_err(|error| format!("failed to decode peer identity: {error}"))?;
    let peer = document
        .into_trusted_peer_identity()
        .map_err(|error| format!("invalid peer identity: {error}"))?;

    store.save_trusted_peer(&peer).map_err(|source| {
        format_identity_store_error("failed to save trusted peer", path, &source)
    })?;

    Ok(IdentityTrustResult {
        trusted: true,
        peer_id: peer.peer_id().to_string(),
        display_name: peer.display_name().to_string(),
        fingerprint: peer.fingerprint().to_string(),
        capabilities: peer.capabilities(),
    })
}

fn forget_trusted_identity_from_path(
    path: &Path,
    display_name: &str,
    peer_id: &str,
) -> Result<IdentityForgetTrustedResult, String> {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() {
        return Err("trusted peer id is required.".to_string());
    }

    let store = FileIdentityStore::new(path);
    store.load_or_create(display_name).map_err(|source| {
        format_identity_store_error("failed to load Akraz identity", path, &source)
    })?;
    store.remove_trusted_peer(peer_id).map_err(|source| {
        format_identity_store_error("failed to remove trusted peer", path, &source)
    })?;

    Ok(IdentityForgetTrustedResult {
        forgotten: true,
        peer_id: peer_id.to_string(),
    })
}

fn default_pairing_capabilities() -> CapabilityFlags {
    CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
}

impl From<TrustedPeerIdentity> for IdentityTrustedPeer {
    fn from(peer: TrustedPeerIdentity) -> Self {
        Self {
            peer_id: peer.peer_id().to_string(),
            display_name: peer.display_name().to_string(),
            fingerprint: peer.fingerprint().to_string(),
            capabilities: peer.capabilities(),
        }
    }
}

fn format_identity_store_error(
    operation: &str,
    path: &Path,
    source: &(dyn std::error::Error + 'static),
) -> String {
    format!("{operation} at {}: {source}", path.display())
}

fn load_settings_from_path(path: &PathBuf) -> Result<AppSettings, String> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let settings = serde_json::from_str::<AppSettings>(&contents)
                .map_err(|error| format!("failed to read Akraz settings: {error}"))?;
            normalize_settings(settings)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(AppSettings::default()),
        Err(error) => Err(format!("failed to read Akraz settings: {error}")),
    }
}

fn save_settings_to_path(path: &PathBuf, settings: AppSettings) -> Result<AppSettings, String> {
    let settings = normalize_settings(settings)?;
    let Some(directory) = path.parent() else {
        return Err("failed to resolve Akraz settings directory.".to_string());
    };

    fs::create_dir_all(directory)
        .map_err(|error| format!("failed to create Akraz settings directory: {error}"))?;
    let contents = serde_json::to_string_pretty(&settings)
        .map_err(|error| format!("failed to encode Akraz settings: {error}"))?;
    fs::write(path, contents).map_err(|error| format!("failed to save Akraz settings: {error}"))?;

    Ok(settings)
}

fn load_layout_from_path(path: &PathBuf) -> Result<LayoutSettings, String> {
    let settings = load_settings_from_path(path)?;
    Ok(LayoutSettings {
        edge_bindings: settings.edge_bindings,
    })
}

fn save_layout_to_path(path: &PathBuf, layout: LayoutSettings) -> Result<LayoutSettings, String> {
    let mut settings = load_settings_from_path(path)?;
    settings.edge_bindings = layout.edge_bindings;
    let saved = save_settings_to_path(path, settings)?;

    Ok(LayoutSettings {
        edge_bindings: saved.edge_bindings,
    })
}

fn normalize_settings(mut settings: AppSettings) -> Result<AppSettings, String> {
    settings.peer_listen_address =
        normalize_optional_session_address(&settings.peer_listen_address)?;
    settings.edge_bindings = settings
        .edge_bindings
        .into_iter()
        .map(normalize_edge_binding)
        .collect::<Result<Vec<_>, _>>()?;
    settings.manual_peer_addresses =
        normalize_manual_peer_addresses(settings.manual_peer_addresses)?;

    Ok(settings)
}

fn normalize_edge_binding(
    mut binding: DaemonEdgeBindingOption,
) -> Result<DaemonEdgeBindingOption, String> {
    binding.peer_id = normalize_peer_id(&binding.peer_id)?.to_string();
    Ok(binding)
}

fn normalize_manual_peer_addresses(
    addresses: Vec<ManualPeerAddressSetting>,
) -> Result<Vec<ManualPeerAddressSetting>, String> {
    let mut normalized_addresses: Vec<ManualPeerAddressSetting> = Vec::new();
    for address in addresses {
        let Some(address) = normalize_manual_peer_address(address)? else {
            continue;
        };

        if let Some(existing) = normalized_addresses
            .iter_mut()
            .find(|existing| existing.peer_id == address.peer_id)
        {
            *existing = address;
        } else {
            normalized_addresses.push(address);
        }
    }

    Ok(normalized_addresses)
}

fn normalize_manual_peer_address(
    mut address: ManualPeerAddressSetting,
) -> Result<Option<ManualPeerAddressSetting>, String> {
    let normalized_address = address.address.trim();
    if normalized_address.is_empty() {
        return Ok(None);
    }

    address.peer_id = normalize_manual_peer_address_peer_id(&address.peer_id)?.to_string();
    address.address = normalize_session_address(normalized_address)?.to_string();
    Ok(Some(address))
}

fn normalize_manual_peer_address_peer_id(peer_id: &str) -> Result<&str, String> {
    let peer_id = normalize_required_session_value("manual peer address peer id", peer_id)?;
    if peer_id.contains(':') || peer_id.contains('@') {
        return Err("manual peer address peer id cannot contain ':' or '@'.".to_string());
    }

    Ok(peer_id)
}

fn normalize_optional_session_address(address: &str) -> Result<String, String> {
    let address = address.trim();
    if address.is_empty() {
        return Ok(String::new());
    }

    Ok(normalize_session_address(address)?.to_string())
}

fn normalize_session_connect_options(
    mut options: SessionConnectOptions,
) -> Result<SessionConnectOptions, String> {
    options.peer_id = normalize_session_peer_id(&options.peer_id)?.to_string();
    options.local_device_id =
        normalize_required_session_value("local device id", &options.local_device_id)?.to_string();
    options.address = normalize_session_address(&options.address)?.to_string();

    Ok(options)
}

fn normalize_session_peer_id(peer_id: &str) -> Result<&str, String> {
    let peer_id = normalize_required_session_value("peer id", peer_id)?;
    if peer_id.contains(':') || peer_id.contains('@') {
        return Err("peer id cannot contain ':' or '@'.".to_string());
    }

    Ok(peer_id)
}

fn normalize_session_address(address: &str) -> Result<&str, String> {
    let address = normalize_required_session_value("address", address)?;
    address
        .parse::<std::net::SocketAddr>()
        .map_err(|_| "address must be an IP address and port.".to_string())?;

    Ok(address)
}

fn normalize_required_session_value<'a>(
    field: &'static str,
    value: &'a str,
) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} is required."));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{field} cannot contain whitespace."));
    }

    Ok(value)
}

#[cfg(test)]
fn parse_daemon_status_response(response_line: &str) -> Result<DaemonStatus, String> {
    parse_json_rpc_response(response_line, "status")
}

fn parse_json_rpc_response<T>(response_line: &str, label: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let value: serde_json::Value = serde_json::from_str(response_line.trim_end())
        .map_err(|error| format!("daemon returned invalid JSON: {error}"))?;

    if value.get("error").is_some() {
        let failure: JsonRpcFailure = serde_json::from_value(value)
            .map_err(|error| format!("daemon returned an invalid error response: {error}"))?;
        return Err(failure.error.message);
    }

    let success: JsonRpcSuccess<T> = serde_json::from_value(value)
        .map_err(|error| format!("daemon returned an invalid {label} response: {error}"))?;
    if success.jsonrpc != JSONRPC_VERSION {
        return Err(format!(
            "daemon returned unsupported JSON-RPC version: {}",
            success.jsonrpc
        ));
    }
    if success.id != LOCAL_REQUEST_ID {
        return Err(format!(
            "daemon returned unexpected response id: {}",
            success.id
        ));
    }

    Ok(success.result)
}

fn classify_daemon_call_error(
    error: &IpcCallError,
    fallback_endpoint: &IpcEndpoint,
) -> DaemonCallFailure {
    match error {
        IpcCallError::Transport {
            source: IpcTransportError::EndpointUnavailable { endpoint, message },
        } => DaemonCallFailure::NotRunning(format!(
            "akraz-daemon is not running at {endpoint}. Details: {message}"
        )),
        IpcCallError::Transport {
            source: IpcTransportError::RequestFailed { message },
        } => DaemonCallFailure::Unreachable(format!(
            "akraz-daemon accepted a connection at {fallback_endpoint}, but did not answer correctly. Details: {message}"
        )),
        IpcCallError::Encode { source } => {
            DaemonCallFailure::Failed(format!("failed to encode daemon IPC request: {source}"))
        }
    }
}

fn spawn_daemon_process(
    app: &tauri::AppHandle,
    options: &DaemonStartOptions,
) -> Result<SpawnedDaemonProcess, String> {
    let identity_store_path = daemon_identity_store_path(app)?;
    let crash_marker_path = daemon_crash_marker_path(app)?;
    if let Some(executable) = resolve_env_daemon_executable() {
        return spawn_os_daemon_process(
            &executable,
            options,
            &identity_store_path,
            &crash_marker_path,
        )
        .map(|child| SpawnedDaemonProcess {
            child: ManagedDaemonChild::Os(child),
            sidecar_events: None,
        });
    }

    match spawn_sidecar_daemon_process(app, options, &identity_store_path, &crash_marker_path) {
        Ok((sidecar_events, child)) => Ok(SpawnedDaemonProcess {
            child: ManagedDaemonChild::Sidecar(child),
            sidecar_events: Some(sidecar_events),
        }),
        Err(sidecar_error) => {
            let executable = resolve_adjacent_daemon_executable()?;
            spawn_os_daemon_process(
                &executable,
                options,
                &identity_store_path,
                &crash_marker_path,
            )
            .map(|child| SpawnedDaemonProcess {
                child: ManagedDaemonChild::Os(child),
                sidecar_events: None,
            })
            .map_err(|fallback_error| {
                format!(
                    "failed to start bundled akraz-daemon sidecar: {sidecar_error}. Also failed to start adjacent daemon at {}: {fallback_error}",
                    executable.display()
                )
            })
        }
    }
}

fn spawn_sidecar_daemon_process(
    app: &tauri::AppHandle,
    options: &DaemonStartOptions,
    identity_store_path: &Path,
    crash_marker_path: &Path,
) -> Result<(Receiver<CommandEvent>, CommandChild), String> {
    app.shell()
        .sidecar(DAEMON_SIDECAR_NAME)
        .map_err(|error| error.to_string())?
        .args(daemon_spawn_args(
            options,
            identity_store_path,
            crash_marker_path,
        )?)
        .spawn()
        .map_err(|error| error.to_string())
}

fn spawn_os_daemon_process(
    executable: &PathBuf,
    options: &DaemonStartOptions,
    identity_store_path: &Path,
    crash_marker_path: &Path,
) -> Result<Child, String> {
    StdCommand::new(executable)
        .args(daemon_spawn_args(
            options,
            identity_store_path,
            crash_marker_path,
        )?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            format!(
                "failed to start akraz-daemon at {}: {error}",
                executable.display()
            )
        })
}

fn daemon_spawn_args(
    options: &DaemonStartOptions,
    identity_store_path: &Path,
    crash_marker_path: &Path,
) -> Result<Vec<String>, String> {
    daemon_spawn_args_from(
        options,
        std::env::var_os(DAEMON_CAPTURE_INPUT_ENV),
        Some(identity_store_path),
        Some(crash_marker_path),
    )
}

fn daemon_spawn_args_from(
    options: &DaemonStartOptions,
    capture_input: Option<OsString>,
    identity_store_path: Option<&Path>,
    crash_marker_path: Option<&Path>,
) -> Result<Vec<String>, String> {
    let mut args = vec![DAEMON_SERVE_ARG.to_string()];
    if let Some(identity_store_path) = identity_store_path {
        args.push(DAEMON_IDENTITY_STORE_ARG.to_string());
        args.push(format_daemon_path_arg(identity_store_path)?);
        args.push(DAEMON_IDENTITY_DISPLAY_NAME_ARG.to_string());
        args.push(DAEMON_IDENTITY_DISPLAY_NAME.to_string());
    }
    if let Some(crash_marker_path) = crash_marker_path {
        args.push(DAEMON_CRASH_MARKER_ARG.to_string());
        args.push(format_daemon_path_arg(crash_marker_path)?);
    }
    if options
        .capture_input
        .unwrap_or_else(|| daemon_capture_input_enabled_from(capture_input))
    {
        args.push(DAEMON_CAPTURE_INPUT_ARG.to_string());
    }
    if let Some(peer_listen_address) = &options.peer_listen_address {
        let peer_listen_address = normalize_optional_session_address(peer_listen_address)?;
        if !peer_listen_address.is_empty() {
            if identity_store_path.is_none() {
                return Err("peer listener requires an identity store.".to_string());
            }
            args.push(DAEMON_PEER_LISTEN_ARG.to_string());
            args.push(peer_listen_address);
        }
    }
    for binding in &options.edge_bindings {
        args.push(DAEMON_EDGE_BINDING_ARG.to_string());
        args.push(format_edge_binding_arg(binding)?);
    }

    Ok(args)
}

fn format_daemon_path_arg(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(ToString::to_string)
        .ok_or_else(|| "Akraz daemon path contains invalid Unicode.".to_string())
}

fn format_edge_binding_arg(binding: &DaemonEdgeBindingOption) -> Result<String, String> {
    let peer_id = normalize_peer_id(&binding.peer_id)?;

    Ok(format!(
        "{}:{}:{}",
        binding.local_edge.daemon_arg(),
        peer_id,
        binding.remote_edge.daemon_arg()
    ))
}

fn normalize_peer_id(peer_id: &str) -> Result<&str, String> {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() {
        return Err("edge binding peer id is required.".to_string());
    }
    if peer_id.contains(':') {
        return Err("edge binding peer id cannot contain ':'.".to_string());
    }

    Ok(peer_id)
}

fn non_empty_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn daemon_capture_input_enabled_from(value: Option<OsString>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let normalized = value.to_string_lossy().trim().to_ascii_lowercase();

    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

fn watch_sidecar_termination(managed: ManagedDaemon, pid: u32, mut events: Receiver<CommandEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            if matches!(event, CommandEvent::Terminated(_)) {
                clear_managed_child_if_pid(&managed, pid);
                break;
            }
        }
    });
}

fn resolve_env_daemon_executable() -> Option<PathBuf> {
    resolve_env_daemon_executable_from(std::env::var_os(DAEMON_PATH_ENV))
}

fn resolve_env_daemon_executable_from(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn resolve_adjacent_daemon_executable() -> Result<PathBuf, String> {
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("failed to locate the Akraz app executable: {error}"))?;
    let Some(directory) = current_exe.parent() else {
        return Err("failed to locate the Akraz app executable directory.".to_string());
    };

    Ok(directory.join(daemon_executable_name()))
}

fn daemon_executable_name() -> &'static str {
    if cfg!(windows) {
        "akraz-daemon.exe"
    } else {
        "akraz-daemon"
    }
}

fn managed_daemon_pid(managed: &ManagedDaemon) -> Result<Option<u32>, String> {
    let mut state = lock_managed_daemon(managed)?;
    let mut clear_child = false;
    let pid = match state.child.as_mut() {
        Some(child) => {
            if child.is_running() {
                Some(child.pid())
            } else {
                clear_child = true;
                None
            }
        }
        None => None,
    };

    if clear_child {
        state.child = None;
    }

    Ok(pid)
}

fn store_managed_child(managed: &ManagedDaemon, child: ManagedDaemonChild) -> Result<u32, String> {
    let mut state = lock_managed_daemon(managed)?;
    let child = child;
    let pid = child.pid();
    let replace_child = match state.child.as_mut() {
        Some(existing) => {
            if existing.is_running() {
                let existing_pid = existing.pid();
                child.kill()?;
                return Ok(existing_pid);
            }

            true
        }
        None => true,
    };

    if replace_child {
        state.child = Some(child);
    }

    Ok(pid)
}

fn take_managed_child(managed: &ManagedDaemon) -> Result<Option<ManagedDaemonChild>, String> {
    let mut state = lock_managed_daemon(managed)?;
    let Some(mut child) = state.child.take() else {
        return Ok(None);
    };

    if child.is_running() {
        Ok(Some(child))
    } else {
        Ok(None)
    }
}

fn clear_managed_child(managed: &ManagedDaemon) -> Result<(), String> {
    let mut state = lock_managed_daemon(managed)?;
    state.child = None;
    Ok(())
}

fn clear_managed_child_if_pid(managed: &ManagedDaemon, pid: u32) {
    let Ok(mut state) = managed.lock() else {
        return;
    };

    if state.child.as_ref().is_some_and(|child| child.pid() == pid) {
        state.child = None;
    }
}

fn lock_managed_daemon(
    managed: &ManagedDaemon,
) -> Result<std::sync::MutexGuard<'_, DaemonProcessState>, String> {
    managed
        .lock()
        .map_err(|_| "daemon process state is unavailable.".to_string())
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::PathBuf;

    use akraz_identity::FileIdentityStore;
    use akraz_ipc::{
        ControlModeSnapshot, DaemonCrashMarker, DaemonCrashMarkerPrivacy, DaemonShutdownResult,
        DaemonStatus, DiagnosticsKeyboardLayout, DiagnosticsMonitorSnapshot,
        DiagnosticsScreenTopology, InputReleaseAllResult, IpcEndpoint, IpcPlatformCapabilities,
        IpcTransportError, JsonRpcError, JsonRpcFailure, JsonRpcSuccess, LogicalPointSnapshot,
        LogicalRectSnapshot, PermissionIssue, PermissionsProbe, ProtocolVersionSnapshot,
        SessionConnectResult, SessionStatus, to_json_line,
    };

    use super::{
        AppSettings, DAEMON_CAPTURE_INPUT_ARG, DAEMON_CRASH_DIR_NAME, DAEMON_CRASH_MARKER_ARG,
        DAEMON_CRASH_MARKER_FILE_NAME, DAEMON_EDGE_BINDING_ARG, DAEMON_IDENTITY_DISPLAY_NAME,
        DAEMON_IDENTITY_DISPLAY_NAME_ARG, DAEMON_IDENTITY_STORE_ARG, DAEMON_LIFECYCLE_SMOKE_FLAG,
        DAEMON_PEER_LISTEN_ARG, DAEMON_SERVE_ARG, DAEMON_SETTINGS_START_SMOKE_FLAG,
        DAEMON_SIDECAR_NAME, DaemonEdgeBindingOption, DaemonLifecyclePhase,
        DaemonLifecycleSmokeReport, DaemonLifecycleSnapshot, DaemonScreenEdgeOption,
        DaemonStartOptions, DaemonStopOutcome, IDENTITY_STORE_DIR_NAME, IDENTITY_STORE_FILE_NAME,
        LayoutSettings, ManualPeerAddressSetting, build_identity_show_result_from_path,
        classify_daemon_call_error, daemon_capture_input_enabled_from,
        daemon_crash_marker_path_from_config_dir, daemon_executable_name, daemon_spawn_args_from,
        default_pairing_capabilities, forget_trusted_identity_from_path, format_edge_binding_arg,
        has_exact_arg, identity_store_path_from_config_dir, list_trusted_identities_from_path,
        load_layout_from_path, load_settings_from_path, normalize_session_connect_options,
        parse_daemon_status_response, parse_json_rpc_response,
        read_previous_daemon_crash_from_path, resolve_env_daemon_executable_from,
        save_layout_to_path, save_settings_to_path, settings_start_smoke_settings,
        trust_identity_document_from_json,
    };

    fn monitor_snapshots() -> Vec<DiagnosticsMonitorSnapshot> {
        vec![DiagnosticsMonitorSnapshot {
            id: "primary".to_string(),
            bounds: LogicalRectSnapshot {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            scale_factor_percent: Some(100),
            is_primary: true,
        }]
    }

    fn keyboard_layout() -> DiagnosticsKeyboardLayout {
        DiagnosticsKeyboardLayout {
            source: "foregroundWindowThread".to_string(),
            layout_id: "0x0000000004120412".to_string(),
            language_id: "0x0412".to_string(),
            layout_name: Some("00000412".to_string()),
        }
    }

    fn status_fixture() -> DaemonStatus {
        DaemonStatus {
            daemon_version: "0.1.0".to_string(),
            mode: ControlModeSnapshot::Local,
            protocol: ProtocolVersionSnapshot { major: 1, minor: 0 },
            peers: Vec::new(),
            capabilities: IpcPlatformCapabilities {
                can_capture_pointer: true,
                can_capture_keyboard: true,
                can_inject_pointer: true,
                can_inject_keyboard: true,
            },
        }
    }

    #[test]
    fn parses_daemon_status_success_response() {
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", status_fixture())) {
            Ok(line) => line,
            Err(error) => panic!("expected status JSON: {error}"),
        };

        assert_eq!(parse_daemon_status_response(&line), Ok(status_fixture()));
    }

    #[test]
    fn parses_daemon_error_response_as_user_message() {
        let line = match to_json_line(&JsonRpcFailure::new(
            Some("tauri".to_string()),
            JsonRpcError::new(-32000, "daemon status unavailable"),
        )) {
            Ok(line) => line,
            Err(error) => panic!("expected failure JSON: {error}"),
        };

        assert_eq!(
            parse_daemon_status_response(&line),
            Err("daemon status unavailable".to_string())
        );
    }

    #[test]
    fn parses_permissions_probe_success_response() {
        let probe = PermissionsProbe {
            adapter_name: "fake".to_string(),
            capabilities: IpcPlatformCapabilities {
                can_capture_pointer: true,
                can_capture_keyboard: false,
                can_inject_pointer: true,
                can_inject_keyboard: false,
            },
            issues: vec![PermissionIssue {
                code: "capture_keyboard_unavailable".to_string(),
                message: "Keyboard capture is not available.".to_string(),
            }],
        };
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", probe.clone())) {
            Ok(line) => line,
            Err(error) => panic!("expected permissions probe JSON: {error}"),
        };

        assert_eq!(
            parse_json_rpc_response::<PermissionsProbe>(&line, "permissions probe"),
            Ok(probe)
        );
    }

    #[test]
    fn parses_diagnostics_screen_topology_success_response() {
        let topology = DiagnosticsScreenTopology {
            pointer_position: LogicalPointSnapshot { x: 1919, y: 540 },
            virtual_screen_bounds: LogicalRectSnapshot {
                x: -1920,
                y: 0,
                width: 3840,
                height: 1080,
            },
            monitors: monitor_snapshots(),
        };
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", topology.clone())) {
            Ok(line) => line,
            Err(error) => panic!("expected diagnostics screen topology JSON: {error}"),
        };

        assert_eq!(
            parse_json_rpc_response::<DiagnosticsScreenTopology>(&line, "screen topology"),
            Ok(topology)
        );
    }

    #[test]
    fn parses_diagnostics_keyboard_layout_success_response() {
        let layout = keyboard_layout();
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", layout.clone())) {
            Ok(line) => line,
            Err(error) => panic!("expected diagnostics keyboard layout JSON: {error}"),
        };

        assert_eq!(
            parse_json_rpc_response::<DiagnosticsKeyboardLayout>(&line, "keyboard layout"),
            Ok(layout)
        );
    }

    #[test]
    fn daemon_call_error_distinguishes_not_running_endpoint() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = akraz_ipc::IpcCallError::Transport {
            source: IpcTransportError::endpoint_unavailable(endpoint.clone(), "not found"),
        };
        let snapshot = classify_daemon_call_error(&error, &endpoint).into_snapshot();

        assert_eq!(snapshot.phase, DaemonLifecyclePhase::NotRunning);
        assert_eq!(snapshot.status, None);
        assert_eq!(
            snapshot.detail,
            Some("akraz-daemon is not running at local-test. Details: not found".to_string())
        );
    }

    #[test]
    fn daemon_call_error_distinguishes_unreachable_endpoint() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = akraz_ipc::IpcCallError::Transport {
            source: IpcTransportError::request_failed("pipe closed"),
        };
        let snapshot = classify_daemon_call_error(&error, &endpoint).into_snapshot();

        assert_eq!(snapshot.phase, DaemonLifecyclePhase::Unreachable);
        assert_eq!(
            snapshot.detail,
            Some(
                "akraz-daemon accepted a connection at local-test, but did not answer correctly. Details: pipe closed"
                    .to_string()
            )
        );
    }

    #[test]
    fn daemon_executable_name_matches_platform() {
        if cfg!(windows) {
            assert_eq!(daemon_executable_name(), "akraz-daemon.exe");
        } else {
            assert_eq!(daemon_executable_name(), "akraz-daemon");
        }
    }

    #[test]
    fn sidecar_name_matches_tauri_external_bin_basename() {
        assert_eq!(DAEMON_SIDECAR_NAME, "akraz-daemon");
    }

    #[test]
    fn daemon_smoke_flags_are_explicit_and_hidden() {
        assert!(has_exact_arg(
            [OsString::from(DAEMON_LIFECYCLE_SMOKE_FLAG)],
            OsStr::new(DAEMON_LIFECYCLE_SMOKE_FLAG)
        ));
        assert!(has_exact_arg(
            [
                OsString::from("--ordinary-open-argument"),
                OsString::from(DAEMON_SETTINGS_START_SMOKE_FLAG)
            ],
            OsStr::new(DAEMON_SETTINGS_START_SMOKE_FLAG)
        ));
        assert!(!has_exact_arg(
            [OsString::from("--smoke-daemon-lifecycle")],
            OsStr::new(DAEMON_LIFECYCLE_SMOKE_FLAG)
        ));
        assert!(!has_exact_arg(
            [OsString::from("document.akraz")],
            OsStr::new(DAEMON_SETTINGS_START_SMOKE_FLAG)
        ));
    }

    #[test]
    fn daemon_path_env_override_ignores_missing_and_empty_values() {
        assert_eq!(resolve_env_daemon_executable_from(None), None);
        assert_eq!(
            resolve_env_daemon_executable_from(Some(OsString::new())),
            None
        );
    }

    #[test]
    fn daemon_path_env_override_accepts_explicit_path() {
        assert_eq!(
            resolve_env_daemon_executable_from(Some(OsString::from("custom-daemon"))),
            Some(PathBuf::from("custom-daemon"))
        );
    }

    #[test]
    fn daemon_capture_input_env_accepts_explicit_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            assert!(daemon_capture_input_enabled_from(Some(OsString::from(
                value
            ))));
        }
        for value in ["", "0", "false", "no", "off", "maybe"] {
            assert!(!daemon_capture_input_enabled_from(Some(OsString::from(
                value
            ))));
        }
        assert!(!daemon_capture_input_enabled_from(None));
    }

    #[test]
    fn daemon_spawn_args_include_capture_only_when_enabled() {
        assert_eq!(
            daemon_spawn_args_from(&DaemonStartOptions::default(), None, None, None),
            Ok(vec![DAEMON_SERVE_ARG.to_string()])
        );
        assert_eq!(
            daemon_spawn_args_from(
                &DaemonStartOptions::default(),
                Some(OsString::from("1")),
                None,
                None
            ),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_CAPTURE_INPUT_ARG.to_string()
            ])
        );
        assert_eq!(
            daemon_spawn_args_from(
                &DaemonStartOptions {
                    capture_input: Some(false),
                    peer_listen_address: None,
                    edge_bindings: Vec::new(),
                },
                Some(OsString::from("1")),
                None,
                None,
            ),
            Ok(vec![DAEMON_SERVE_ARG.to_string()])
        );
    }

    #[test]
    fn daemon_spawn_args_include_identity_store_when_configured() {
        let identity_store_path = PathBuf::from("akraz-identity.json");

        assert_eq!(
            daemon_spawn_args_from(
                &DaemonStartOptions::default(),
                None,
                Some(&identity_store_path),
                None
            ),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_IDENTITY_STORE_ARG.to_string(),
                "akraz-identity.json".to_string(),
                DAEMON_IDENTITY_DISPLAY_NAME_ARG.to_string(),
                DAEMON_IDENTITY_DISPLAY_NAME.to_string()
            ])
        );
    }

    #[test]
    fn daemon_spawn_args_include_crash_marker_when_configured() {
        let crash_marker_path = PathBuf::from("daemon-crash.json");

        assert_eq!(
            daemon_spawn_args_from(
                &DaemonStartOptions::default(),
                None,
                None,
                Some(&crash_marker_path)
            ),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_CRASH_MARKER_ARG.to_string(),
                "daemon-crash.json".to_string()
            ])
        );
    }

    #[test]
    fn daemon_spawn_args_include_configured_edge_bindings() {
        let options = DaemonStartOptions {
            capture_input: Some(true),
            peer_listen_address: None,
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: " linux-laptop ".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
        };

        assert_eq!(
            daemon_spawn_args_from(&options, None, None, None),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_CAPTURE_INPUT_ARG.to_string(),
                DAEMON_EDGE_BINDING_ARG.to_string(),
                "right:linux-laptop:left".to_string()
            ])
        );
    }

    #[test]
    fn daemon_spawn_args_include_configured_peer_listener() {
        let identity_store_path = PathBuf::from("akraz-identity.json");
        let options = DaemonStartOptions {
            capture_input: Some(false),
            peer_listen_address: Some(" 127.0.0.1:4455 ".to_string()),
            edge_bindings: Vec::new(),
        };

        assert_eq!(
            daemon_spawn_args_from(&options, None, Some(&identity_store_path), None),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_IDENTITY_STORE_ARG.to_string(),
                "akraz-identity.json".to_string(),
                DAEMON_IDENTITY_DISPLAY_NAME_ARG.to_string(),
                DAEMON_IDENTITY_DISPLAY_NAME.to_string(),
                DAEMON_PEER_LISTEN_ARG.to_string(),
                "127.0.0.1:4455".to_string()
            ])
        );
        assert_eq!(
            daemon_spawn_args_from(&options, None, None, None),
            Err("peer listener requires an identity store.".to_string())
        );
    }

    #[test]
    fn identity_store_path_lives_under_config_secrets_directory() {
        assert_eq!(
            identity_store_path_from_config_dir(PathBuf::from("akraz-config")),
            PathBuf::from("akraz-config")
                .join(IDENTITY_STORE_DIR_NAME)
                .join(IDENTITY_STORE_FILE_NAME)
        );
    }

    #[test]
    fn daemon_crash_marker_path_lives_under_config_crash_directory() {
        assert_eq!(
            daemon_crash_marker_path_from_config_dir(PathBuf::from("akraz-config")),
            PathBuf::from("akraz-config")
                .join(DAEMON_CRASH_DIR_NAME)
                .join(DAEMON_CRASH_MARKER_FILE_NAME)
        );
    }

    #[test]
    fn reads_previous_daemon_crash_marker_from_path() {
        let path = unique_identity_path("daemon-crash-marker");
        let marker = DaemonCrashMarker {
            schema_version: "akraz.daemonCrashMarker/v1".to_string(),
            process_role: "akraz-daemon".to_string(),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            reason: "panic".to_string(),
            panic_message_class: "stringPayload".to_string(),
            panic_location: None,
            recorded_at_unix_millis: 123_456,
            privacy: DaemonCrashMarkerPrivacy::default(),
        };
        fs::create_dir_all(path.parent().expect("crash marker parent"))
            .expect("crash marker parent directory");
        fs::write(
            &path,
            serde_json::to_string(&marker).expect("encoded crash marker"),
        )
        .expect("crash marker file");

        let decoded = read_previous_daemon_crash_from_path(&path).expect("previous crash marker");
        let _ = fs::remove_file(&path);

        assert_eq!(decoded, Some(marker));
    }

    #[test]
    fn identity_show_exports_public_pairing_document_without_secret_key() {
        let path = unique_identity_path("show");

        let result =
            build_identity_show_result_from_path(&path, " Windows Desktop ").expect("identity");

        assert_eq!(result.display_name, "Windows Desktop");
        assert_eq!(result.capabilities, default_pairing_capabilities());
        assert!(
            result
                .document_json
                .contains("\"kind\": \"akraz.peerIdentity\"")
        );
        assert!(result.document_json.contains(&result.device_id));
        assert!(result.document_json.contains(&result.fingerprint));
        assert!(!result.document_json.contains("identitySecretKey"));

        remove_identity_path(path);
    }

    #[test]
    fn identity_trust_saves_valid_peer_document() {
        let source_path = unique_identity_path("source");
        let target_path = unique_identity_path("target");
        let source = build_identity_show_result_from_path(&source_path, "Source Laptop")
            .expect("source identity");

        let result = trust_identity_document_from_json(
            &target_path,
            "Target Desktop",
            &source.document_json,
        )
        .expect("trusted peer");

        assert!(result.trusted);
        assert_eq!(result.peer_id, source.device_id);
        assert_eq!(result.display_name, "Source Laptop");
        assert_eq!(result.fingerprint, source.fingerprint);
        assert_eq!(result.capabilities, default_pairing_capabilities());

        let loaded = FileIdentityStore::new(&target_path)
            .load_trusted_peer(&result.peer_id)
            .expect("loaded trusted peer");
        assert_eq!(loaded.identity().peer_id(), result.peer_id);
        assert_eq!(loaded.identity().display_name(), result.display_name);
        assert_eq!(loaded.identity().fingerprint(), result.fingerprint);

        remove_identity_path(source_path);
        remove_identity_path(target_path);
    }

    #[test]
    fn identity_list_trusted_returns_saved_peers() {
        let source_path = unique_identity_path("source-list");
        let target_path = unique_identity_path("target-list");
        let source = build_identity_show_result_from_path(&source_path, "Source Laptop")
            .expect("source identity");

        trust_identity_document_from_json(&target_path, "Target Desktop", &source.document_json)
            .expect("trusted peer");

        let result = list_trusted_identities_from_path(&target_path, "Target Desktop")
            .expect("trusted peers");

        assert_eq!(result.peers.len(), 1);
        assert_eq!(result.peers[0].peer_id, source.device_id);
        assert_eq!(result.peers[0].display_name, "Source Laptop");
        assert_eq!(result.peers[0].fingerprint, source.fingerprint);
        assert_eq!(result.peers[0].capabilities, default_pairing_capabilities());

        remove_identity_path(source_path);
        remove_identity_path(target_path);
    }

    #[test]
    fn identity_forget_trusted_removes_saved_peer() {
        let source_path = unique_identity_path("source-forget");
        let target_path = unique_identity_path("target-forget");
        let source = build_identity_show_result_from_path(&source_path, "Source Laptop")
            .expect("source identity");
        trust_identity_document_from_json(&target_path, "Target Desktop", &source.document_json)
            .expect("trusted peer");

        let result =
            forget_trusted_identity_from_path(&target_path, "Target Desktop", &source.device_id)
                .expect("forgot trusted peer");

        assert!(result.forgotten);
        assert_eq!(result.peer_id, source.device_id);
        assert!(
            list_trusted_identities_from_path(&target_path, "Target Desktop")
                .expect("trusted peers")
                .peers
                .is_empty()
        );

        remove_identity_path(source_path);
        remove_identity_path(target_path);
    }

    #[test]
    fn identity_forget_trusted_rejects_empty_peer_id() {
        let path = unique_identity_path("empty-forget-peer");

        assert_eq!(
            forget_trusted_identity_from_path(&path, "Target Desktop", " "),
            Err("trusted peer id is required.".to_string())
        );

        remove_identity_path(path);
    }

    #[test]
    fn identity_trust_rejects_empty_peer_document() {
        let path = unique_identity_path("empty-peer");

        assert_eq!(
            trust_identity_document_from_json(&path, "Target Desktop", " "),
            Err("peer identity is required.".to_string())
        );

        remove_identity_path(path);
    }

    #[test]
    fn settings_load_defaults_when_file_is_missing() {
        let path = unique_settings_path("missing");

        assert_eq!(load_settings_from_path(&path), Ok(AppSettings::default()));
    }

    #[test]
    fn settings_save_normalizes_and_loads_edge_bindings() {
        let path = unique_settings_path("roundtrip");
        let settings = AppSettings {
            capture_input: true,
            peer_listen_address: " 127.0.0.1:4455 ".to_string(),
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: " linux-laptop ".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
            manual_peer_addresses: vec![
                ManualPeerAddressSetting {
                    peer_id: " linux-laptop ".to_string(),
                    address: " 127.0.0.1:4455 ".to_string(),
                },
                ManualPeerAddressSetting {
                    peer_id: "empty-address".to_string(),
                    address: " ".to_string(),
                },
                ManualPeerAddressSetting {
                    peer_id: "linux-laptop".to_string(),
                    address: "127.0.0.1:4456".to_string(),
                },
            ],
        };

        let saved = save_settings_to_path(&path, settings).expect("save settings");

        assert_eq!(
            saved,
            AppSettings {
                capture_input: true,
                peer_listen_address: "127.0.0.1:4455".to_string(),
                edge_bindings: vec![DaemonEdgeBindingOption {
                    local_edge: DaemonScreenEdgeOption::Right,
                    peer_id: "linux-laptop".to_string(),
                    remote_edge: DaemonScreenEdgeOption::Left,
                }],
                manual_peer_addresses: vec![ManualPeerAddressSetting {
                    peer_id: "linux-laptop".to_string(),
                    address: "127.0.0.1:4456".to_string(),
                }],
            }
        );
        assert_eq!(load_settings_from_path(&path), Ok(saved));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_save_rejects_edge_bindings_that_break_daemon_cli_contract() {
        let path = unique_settings_path("invalid");
        let settings = AppSettings {
            capture_input: true,
            peer_listen_address: String::new(),
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: "linux:laptop".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
            manual_peer_addresses: Vec::new(),
        };

        assert_eq!(
            save_settings_to_path(&path, settings),
            Err("edge binding peer id cannot contain ':'.".to_string())
        );
    }

    #[test]
    fn layout_set_updates_edge_bindings_without_replacing_other_settings() {
        let path = unique_settings_path("layout-roundtrip");
        let original = AppSettings {
            capture_input: true,
            peer_listen_address: "127.0.0.1:4455".to_string(),
            edge_bindings: Vec::new(),
            manual_peer_addresses: vec![ManualPeerAddressSetting {
                peer_id: "linux-laptop".to_string(),
                address: "127.0.0.1:4456".to_string(),
            }],
        };
        save_settings_to_path(&path, original).expect("save original settings");

        let layout = LayoutSettings {
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: " linux-laptop ".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
        };
        let saved_layout = save_layout_to_path(&path, layout).expect("save layout");

        assert_eq!(
            saved_layout,
            LayoutSettings {
                edge_bindings: vec![DaemonEdgeBindingOption {
                    local_edge: DaemonScreenEdgeOption::Right,
                    peer_id: "linux-laptop".to_string(),
                    remote_edge: DaemonScreenEdgeOption::Left,
                }],
            }
        );
        assert_eq!(load_layout_from_path(&path), Ok(saved_layout.clone()));
        assert_eq!(
            load_settings_from_path(&path),
            Ok(AppSettings {
                capture_input: true,
                peer_listen_address: "127.0.0.1:4455".to_string(),
                edge_bindings: saved_layout.edge_bindings,
                manual_peer_addresses: vec![ManualPeerAddressSetting {
                    peer_id: "linux-laptop".to_string(),
                    address: "127.0.0.1:4456".to_string(),
                }],
            })
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn layout_set_rejects_edge_bindings_that_break_daemon_cli_contract() {
        let path = unique_settings_path("layout-invalid");

        assert_eq!(
            save_layout_to_path(
                &path,
                LayoutSettings {
                    edge_bindings: vec![DaemonEdgeBindingOption {
                        local_edge: DaemonScreenEdgeOption::Right,
                        peer_id: "linux:laptop".to_string(),
                        remote_edge: DaemonScreenEdgeOption::Left,
                    }],
                }
            ),
            Err("edge binding peer id cannot contain ':'.".to_string())
        );
    }

    #[test]
    fn settings_save_rejects_manual_peer_addresses_that_break_session_contract() {
        let path = unique_settings_path("invalid-manual-address");

        assert_eq!(
            save_settings_to_path(
                &path,
                AppSettings {
                    capture_input: false,
                    peer_listen_address: String::new(),
                    edge_bindings: Vec::new(),
                    manual_peer_addresses: vec![ManualPeerAddressSetting {
                        peer_id: "linux:laptop".to_string(),
                        address: "127.0.0.1:4455".to_string(),
                    }],
                }
            ),
            Err("manual peer address peer id cannot contain ':' or '@'.".to_string())
        );
        assert_eq!(
            save_settings_to_path(
                &path,
                AppSettings {
                    capture_input: false,
                    peer_listen_address: String::new(),
                    edge_bindings: Vec::new(),
                    manual_peer_addresses: vec![ManualPeerAddressSetting {
                        peer_id: "linux-laptop".to_string(),
                        address: "localhost:4455".to_string(),
                    }],
                }
            ),
            Err("address must be an IP address and port.".to_string())
        );
    }

    #[test]
    fn session_connect_options_are_trimmed_before_ipc() {
        let options = super::SessionConnectOptions {
            peer_id: " linux-laptop ".to_string(),
            local_device_id: " windows-desktop ".to_string(),
            address: " 127.0.0.1:4455 ".to_string(),
        };

        assert_eq!(
            normalize_session_connect_options(options),
            Ok(super::SessionConnectOptions {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:4455".to_string(),
            })
        );
    }

    #[test]
    fn session_connect_options_reject_invalid_values() {
        assert_eq!(
            normalize_session_connect_options(super::SessionConnectOptions {
                peer_id: "linux:laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:4455".to_string(),
            }),
            Err("peer id cannot contain ':' or '@'.".to_string())
        );
        assert_eq!(
            normalize_session_connect_options(super::SessionConnectOptions {
                peer_id: "linux-laptop".to_string(),
                local_device_id: " ".to_string(),
                address: "127.0.0.1:4455".to_string(),
            }),
            Err("local device id is required.".to_string())
        );
        assert_eq!(
            normalize_session_connect_options(super::SessionConnectOptions {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "localhost:4455".to_string(),
            }),
            Err("address must be an IP address and port.".to_string())
        );
    }

    #[test]
    fn parses_session_connect_success_response() {
        let result = SessionConnectResult {
            connected: true,
            session: SessionStatus {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:4455".to_string(),
                connected: true,
            },
        };
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", result.clone())) {
            Ok(line) => line,
            Err(error) => panic!("expected session connect JSON: {error}"),
        };

        assert_eq!(
            parse_json_rpc_response::<SessionConnectResult>(&line, "session connect"),
            Ok(result)
        );
    }

    #[test]
    fn parses_input_release_all_success_response() {
        let result = InputReleaseAllResult {
            released: true,
            mode: ControlModeSnapshot::Local,
        };
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", result.clone())) {
            Ok(line) => line,
            Err(error) => panic!("expected input release all JSON: {error}"),
        };

        assert_eq!(
            parse_json_rpc_response::<InputReleaseAllResult>(&line, "input release all"),
            Ok(result)
        );
    }

    #[test]
    fn parses_daemon_shutdown_success_response() {
        let result = DaemonShutdownResult {
            requested: true,
            released_inputs: true,
            disconnected_peer_session: false,
            mode: ControlModeSnapshot::Local,
        };
        let line = match to_json_line(&JsonRpcSuccess::new("tauri", result.clone())) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon shutdown JSON: {error}"),
        };

        assert_eq!(
            parse_json_rpc_response::<DaemonShutdownResult>(&line, "daemon shutdown"),
            Ok(result)
        );
    }

    #[test]
    fn daemon_lifecycle_smoke_report_records_graceful_stop_evidence() {
        let shutdown = DaemonShutdownResult {
            requested: true,
            released_inputs: true,
            disconnected_peer_session: false,
            mode: ControlModeSnapshot::Local,
        };
        let mut report =
            DaemonLifecycleSmokeReport::new(DaemonLifecycleSnapshot::not_running("initial"));

        report.started = Some(DaemonLifecycleSnapshot::running(status_fixture(), Some(42)));
        report.record_stop(DaemonStopOutcome::graceful(
            DaemonLifecycleSnapshot::not_running("Akraz stopped."),
            shutdown,
        ));

        let value = serde_json::to_value(&report).expect("smoke report JSON");
        assert_eq!(value["started"]["phase"], "running");
        assert_eq!(value["stopped"]["phase"], "not_running");
        assert_eq!(value["stopMethod"], "graceful_shutdown");
        assert_eq!(value["shutdown"]["requested"], true);
        assert_eq!(value["shutdown"]["releasedInputs"], true);
        assert_eq!(value["shutdown"]["disconnectedPeerSession"], false);
    }

    #[test]
    fn settings_start_smoke_settings_become_daemon_start_options() {
        let identity_store_path = PathBuf::from("akraz-identity.json");
        let options = DaemonStartOptions::from(settings_start_smoke_settings());

        assert_eq!(
            daemon_spawn_args_from(&options, None, Some(&identity_store_path), None),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_IDENTITY_STORE_ARG.to_string(),
                "akraz-identity.json".to_string(),
                DAEMON_IDENTITY_DISPLAY_NAME_ARG.to_string(),
                DAEMON_IDENTITY_DISPLAY_NAME.to_string(),
                DAEMON_CAPTURE_INPUT_ARG.to_string(),
                DAEMON_PEER_LISTEN_ARG.to_string(),
                "127.0.0.1:0".to_string(),
                DAEMON_EDGE_BINDING_ARG.to_string(),
                "right:linux-laptop:left".to_string()
            ])
        );
    }

    #[test]
    fn edge_binding_arg_rejects_values_that_break_daemon_cli_contract() {
        assert_eq!(
            format_edge_binding_arg(&DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: " ".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }),
            Err("edge binding peer id is required.".to_string())
        );
        assert_eq!(
            format_edge_binding_arg(&DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: "linux:laptop".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }),
            Err("edge binding peer id cannot contain ':'.".to_string())
        );
    }

    fn unique_settings_path(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);

        std::env::temp_dir()
            .join(format!(
                "akraz-settings-test-{label}-{}",
                std::process::id()
            ))
            .join(format!("{nanos}.json"))
    }

    fn unique_identity_path(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);

        std::env::temp_dir()
            .join(format!(
                "akraz-app-identity-test-{label}-{}",
                std::process::id()
            ))
            .join(format!("{nanos}.json"))
    }

    fn remove_identity_path(path: PathBuf) {
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
