use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use akraz_ipc::{
    DaemonStatus, DaemonStatusParams, IpcCallError, IpcEndpoint, IpcTransportError,
    JSONRPC_VERSION, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcClient,
    METHOD_DAEMON_STATUS, OsLocalIpcClient, call_json_rpc, resolve_current_default_endpoint,
};
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
const DAEMON_LIFECYCLE_SMOKE_FLAG: &str = "--akraz-smoke-daemon-lifecycle";
const DAEMON_SETTINGS_START_SMOKE_FLAG: &str = "--akraz-smoke-settings-start";
const SETTINGS_FILE_NAME: &str = "settings.json";
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
            daemon_start,
            daemon_stop,
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
        report.stopped = Some(stop_daemon(&managed)?);
        print_smoke_report(&report)?;
        return Err(format!(
            "daemon lifecycle smoke expected running after start, got {:?}",
            started.phase
        ));
    }

    let stopped = stop_daemon(&managed)?;
    report.stopped = Some(stopped.clone());
    print_smoke_report(&report)?;
    if stopped.phase == DaemonLifecyclePhase::Running {
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
        report.stopped = Some(stop_daemon(&managed)?);
        print_smoke_report(&report)?;
        return Err(format!(
            "daemon settings smoke expected running after configured start, got {:?}",
            started.phase
        ));
    }

    let stopped = stop_daemon(&managed)?;
    report.stopped = Some(stopped.clone());
    print_smoke_report(&report)?;
    if stopped.phase == DaemonLifecyclePhase::Running {
        return Err(
            "daemon settings smoke expected daemon to stop, but it is still running.".to_string(),
        );
    }

    Ok(())
}

fn settings_start_smoke_settings() -> AppSettings {
    AppSettings {
        capture_input: true,
        edge_bindings: vec![DaemonEdgeBindingOption {
            local_edge: DaemonScreenEdgeOption::Right,
            peer_id: "linux-laptop".to_string(),
            remote_edge: DaemonScreenEdgeOption::Left,
        }],
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
    started: Option<DaemonLifecycleSnapshot>,
    stopped: Option<DaemonLifecycleSnapshot>,
}

impl DaemonLifecycleSmokeReport {
    fn new(initial: DaemonLifecycleSnapshot) -> Self {
        Self {
            initial,
            settings: None,
            started: None,
            stopped: None,
        }
    }
}

#[tauri::command]
async fn daemon_status(
    managed: tauri::State<'_, ManagedDaemon>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    tauri::async_runtime::spawn_blocking(move || refresh_daemon_snapshot(&managed))
        .await
        .map_err(|error| format!("daemon status task failed: {error}"))?
}

#[tauri::command]
async fn daemon_start(
    app: tauri::AppHandle,
    managed: tauri::State<'_, ManagedDaemon>,
    options: Option<DaemonStartOptions>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    let options = options.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || start_daemon(&app, &managed, options))
        .await
        .map_err(|error| format!("daemon start task failed: {error}"))?
}

#[tauri::command]
async fn daemon_stop(
    managed: tauri::State<'_, ManagedDaemon>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    tauri::async_runtime::spawn_blocking(move || stop_daemon(&managed))
        .await
        .map_err(|error| format!("daemon stop task failed: {error}"))?
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
    edge_bindings: Vec<DaemonEdgeBindingOption>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DaemonStartOptions {
    capture_input: Option<bool>,
    #[serde(default)]
    edge_bindings: Vec<DaemonEdgeBindingOption>,
}

impl From<AppSettings> for DaemonStartOptions {
    fn from(settings: AppSettings) -> Self {
        Self {
            capture_input: Some(settings.capture_input),
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
}

impl DaemonLifecycleSnapshot {
    fn running(status: DaemonStatus, managed_pid: Option<u32>) -> Self {
        Self {
            phase: DaemonLifecyclePhase::Running,
            status: Some(status),
            detail: None,
            managed_pid,
        }
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

fn stop_daemon(managed: &ManagedDaemon) -> Result<DaemonLifecycleSnapshot, String> {
    let Some(child) = take_managed_child(managed)? else {
        return Ok(read_daemon_snapshot()
            .with_detail("This app did not start the current Akraz background process."));
    };

    if let Err(error) = child.kill() {
        return Ok(DaemonLifecycleSnapshot::failed(error));
    }

    Ok(wait_for_stopped_daemon_snapshot())
}

fn read_daemon_snapshot() -> DaemonLifecycleSnapshot {
    match call_daemon_status() {
        Ok(status) => DaemonLifecycleSnapshot::running(status, None),
        Err(snapshot) => snapshot,
    }
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
    let endpoint = match resolve_current_default_endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => return Err(DaemonLifecycleSnapshot::failed(error.to_string())),
    };
    let client = OsLocalIpcClient::new(endpoint);
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| classify_daemon_call_error(&error, client.endpoint()))?;

    parse_daemon_status_response(&response_line).map_err(DaemonLifecycleSnapshot::failed)
}

fn load_app_settings(app: &tauri::AppHandle) -> Result<AppSettings, String> {
    let path = app_settings_path(app)?;
    load_settings_from_path(&path)
}

fn save_app_settings(app: &tauri::AppHandle, settings: AppSettings) -> Result<AppSettings, String> {
    let path = app_settings_path(app)?;
    save_settings_to_path(&path, settings)
}

fn app_settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("failed to resolve Akraz settings directory: {error}"))?;

    Ok(directory.join(SETTINGS_FILE_NAME))
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

fn normalize_settings(mut settings: AppSettings) -> Result<AppSettings, String> {
    settings.edge_bindings = settings
        .edge_bindings
        .into_iter()
        .map(normalize_edge_binding)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(settings)
}

fn normalize_edge_binding(
    mut binding: DaemonEdgeBindingOption,
) -> Result<DaemonEdgeBindingOption, String> {
    binding.peer_id = normalize_peer_id(&binding.peer_id)?.to_string();
    Ok(binding)
}

fn parse_daemon_status_response(response_line: &str) -> Result<DaemonStatus, String> {
    let value: serde_json::Value = serde_json::from_str(response_line.trim_end())
        .map_err(|error| format!("daemon returned invalid JSON: {error}"))?;

    if value.get("error").is_some() {
        let failure: JsonRpcFailure = serde_json::from_value(value)
            .map_err(|error| format!("daemon returned an invalid error response: {error}"))?;
        return Err(failure.error.message);
    }

    let success: JsonRpcSuccess<DaemonStatus> = serde_json::from_value(value)
        .map_err(|error| format!("daemon returned an invalid status response: {error}"))?;
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
) -> DaemonLifecycleSnapshot {
    match error {
        IpcCallError::Transport {
            source: IpcTransportError::EndpointUnavailable { endpoint, message },
        } => DaemonLifecycleSnapshot::not_running(format!(
            "akraz-daemon is not running at {endpoint}. Details: {message}"
        )),
        IpcCallError::Transport {
            source: IpcTransportError::RequestFailed { message },
        } => DaemonLifecycleSnapshot::unreachable(format!(
            "akraz-daemon accepted a connection at {fallback_endpoint}, but did not answer correctly. Details: {message}"
        )),
        IpcCallError::Encode { source } => DaemonLifecycleSnapshot::failed(format!(
            "failed to encode daemon IPC request: {source}"
        )),
    }
}

fn spawn_daemon_process(
    app: &tauri::AppHandle,
    options: &DaemonStartOptions,
) -> Result<SpawnedDaemonProcess, String> {
    if let Some(executable) = resolve_env_daemon_executable() {
        return spawn_os_daemon_process(&executable, options).map(|child| SpawnedDaemonProcess {
            child: ManagedDaemonChild::Os(child),
            sidecar_events: None,
        });
    }

    match spawn_sidecar_daemon_process(app, options) {
        Ok((sidecar_events, child)) => Ok(SpawnedDaemonProcess {
            child: ManagedDaemonChild::Sidecar(child),
            sidecar_events: Some(sidecar_events),
        }),
        Err(sidecar_error) => {
            let executable = resolve_adjacent_daemon_executable()?;
            spawn_os_daemon_process(&executable, options)
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
) -> Result<(Receiver<CommandEvent>, CommandChild), String> {
    app.shell()
        .sidecar(DAEMON_SIDECAR_NAME)
        .map_err(|error| error.to_string())?
        .args(daemon_spawn_args(options)?)
        .spawn()
        .map_err(|error| error.to_string())
}

fn spawn_os_daemon_process(
    executable: &PathBuf,
    options: &DaemonStartOptions,
) -> Result<Child, String> {
    StdCommand::new(executable)
        .args(daemon_spawn_args(options)?)
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

fn daemon_spawn_args(options: &DaemonStartOptions) -> Result<Vec<String>, String> {
    daemon_spawn_args_from(options, std::env::var_os(DAEMON_CAPTURE_INPUT_ENV))
}

fn daemon_spawn_args_from(
    options: &DaemonStartOptions,
    capture_input: Option<OsString>,
) -> Result<Vec<String>, String> {
    let mut args = vec![DAEMON_SERVE_ARG.to_string()];
    if options
        .capture_input
        .unwrap_or_else(|| daemon_capture_input_enabled_from(capture_input))
    {
        args.push(DAEMON_CAPTURE_INPUT_ARG.to_string());
    }
    for binding in &options.edge_bindings {
        args.push(DAEMON_EDGE_BINDING_ARG.to_string());
        args.push(format_edge_binding_arg(binding)?);
    }

    Ok(args)
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
    use std::path::PathBuf;

    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, IpcEndpoint, IpcPlatformCapabilities, IpcTransportError,
        JsonRpcError, JsonRpcFailure, JsonRpcSuccess, ProtocolVersionSnapshot, to_json_line,
    };

    use super::{
        AppSettings, DAEMON_CAPTURE_INPUT_ARG, DAEMON_EDGE_BINDING_ARG,
        DAEMON_LIFECYCLE_SMOKE_FLAG, DAEMON_SERVE_ARG, DAEMON_SETTINGS_START_SMOKE_FLAG,
        DAEMON_SIDECAR_NAME, DaemonEdgeBindingOption, DaemonLifecyclePhase, DaemonScreenEdgeOption,
        DaemonStartOptions, classify_daemon_call_error, daemon_capture_input_enabled_from,
        daemon_executable_name, daemon_spawn_args_from, format_edge_binding_arg, has_exact_arg,
        load_settings_from_path, parse_daemon_status_response, resolve_env_daemon_executable_from,
        save_settings_to_path, settings_start_smoke_settings,
    };

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
    fn daemon_call_error_distinguishes_not_running_endpoint() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = akraz_ipc::IpcCallError::Transport {
            source: IpcTransportError::endpoint_unavailable(endpoint.clone(), "not found"),
        };
        let snapshot = classify_daemon_call_error(&error, &endpoint);

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
        let snapshot = classify_daemon_call_error(&error, &endpoint);

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
            daemon_spawn_args_from(&DaemonStartOptions::default(), None),
            Ok(vec![DAEMON_SERVE_ARG.to_string()])
        );
        assert_eq!(
            daemon_spawn_args_from(&DaemonStartOptions::default(), Some(OsString::from("1"))),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_CAPTURE_INPUT_ARG.to_string()
            ])
        );
        assert_eq!(
            daemon_spawn_args_from(
                &DaemonStartOptions {
                    capture_input: Some(false),
                    edge_bindings: Vec::new(),
                },
                Some(OsString::from("1")),
            ),
            Ok(vec![DAEMON_SERVE_ARG.to_string()])
        );
    }

    #[test]
    fn daemon_spawn_args_include_configured_edge_bindings() {
        let options = DaemonStartOptions {
            capture_input: Some(true),
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: " linux-laptop ".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
        };

        assert_eq!(
            daemon_spawn_args_from(&options, None),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_CAPTURE_INPUT_ARG.to_string(),
                DAEMON_EDGE_BINDING_ARG.to_string(),
                "right:linux-laptop:left".to_string()
            ])
        );
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
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: " linux-laptop ".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
        };

        let saved = save_settings_to_path(&path, settings).expect("save settings");

        assert_eq!(
            saved,
            AppSettings {
                capture_input: true,
                edge_bindings: vec![DaemonEdgeBindingOption {
                    local_edge: DaemonScreenEdgeOption::Right,
                    peer_id: "linux-laptop".to_string(),
                    remote_edge: DaemonScreenEdgeOption::Left,
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
            edge_bindings: vec![DaemonEdgeBindingOption {
                local_edge: DaemonScreenEdgeOption::Right,
                peer_id: "linux:laptop".to_string(),
                remote_edge: DaemonScreenEdgeOption::Left,
            }],
        };

        assert_eq!(
            save_settings_to_path(&path, settings),
            Err("edge binding peer id cannot contain ':'.".to_string())
        );
    }

    #[test]
    fn settings_start_smoke_settings_become_daemon_start_options() {
        let options = DaemonStartOptions::from(settings_start_smoke_settings());

        assert_eq!(
            daemon_spawn_args_from(&options, None),
            Ok(vec![
                DAEMON_SERVE_ARG.to_string(),
                DAEMON_CAPTURE_INPUT_ARG.to_string(),
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
}
