use std::ffi::{OsStr, OsString};
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
use serde::Serialize;
use tauri::async_runtime::Receiver;
use tauri_plugin_shell::{
    ShellExt,
    process::{CommandChild, CommandEvent},
};

const LOCAL_REQUEST_ID: &str = "tauri";
const DAEMON_PATH_ENV: &str = "AKRAZ_DAEMON_PATH";
const DAEMON_SIDECAR_NAME: &str = "akraz-daemon";
const DAEMON_LIFECYCLE_SMOKE_FLAG: &str = "--akraz-smoke-daemon-lifecycle";
const DAEMON_START_RETRIES: usize = 50;
const DAEMON_START_RETRY_DELAY: Duration = Duration::from_millis(40);
const DAEMON_STOP_RETRIES: usize = 50;
const DAEMON_STOP_RETRY_DELAY: Duration = Duration::from_millis(40);

type ManagedDaemon = Arc<Mutex<DaemonProcessState>>;

pub fn run() -> Result<(), String> {
    if should_run_daemon_lifecycle_smoke(std::env::args_os().skip(1)) {
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
            daemon_stop
        ])
}

fn should_run_daemon_lifecycle_smoke<I>(args: I) -> bool
where
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    args.into_iter()
        .any(|argument| argument.as_ref() == OsStr::new(DAEMON_LIFECYCLE_SMOKE_FLAG))
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

    let started = start_daemon(&handle, &managed)?;
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
    started: Option<DaemonLifecycleSnapshot>,
    stopped: Option<DaemonLifecycleSnapshot>,
}

impl DaemonLifecycleSmokeReport {
    fn new(initial: DaemonLifecycleSnapshot) -> Self {
        Self {
            initial,
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
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    tauri::async_runtime::spawn_blocking(move || start_daemon(&app, &managed))
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

    let spawned = match spawn_daemon_process(app) {
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

fn spawn_daemon_process(app: &tauri::AppHandle) -> Result<SpawnedDaemonProcess, String> {
    if let Some(executable) = resolve_env_daemon_executable() {
        return spawn_os_daemon_process(&executable).map(|child| SpawnedDaemonProcess {
            child: ManagedDaemonChild::Os(child),
            sidecar_events: None,
        });
    }

    match spawn_sidecar_daemon_process(app) {
        Ok((sidecar_events, child)) => Ok(SpawnedDaemonProcess {
            child: ManagedDaemonChild::Sidecar(child),
            sidecar_events: Some(sidecar_events),
        }),
        Err(sidecar_error) => {
            let executable = resolve_adjacent_daemon_executable()?;
            spawn_os_daemon_process(&executable)
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
) -> Result<(Receiver<CommandEvent>, CommandChild), String> {
    app.shell()
        .sidecar(DAEMON_SIDECAR_NAME)
        .map_err(|error| error.to_string())?
        .args(["--serve"])
        .spawn()
        .map_err(|error| error.to_string())
}

fn spawn_os_daemon_process(executable: &PathBuf) -> Result<Child, String> {
    StdCommand::new(executable)
        .arg("--serve")
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
    use std::ffi::OsString;
    use std::path::PathBuf;

    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, IpcEndpoint, IpcPlatformCapabilities, IpcTransportError,
        JsonRpcError, JsonRpcFailure, JsonRpcSuccess, ProtocolVersionSnapshot, to_json_line,
    };

    use super::{
        DAEMON_LIFECYCLE_SMOKE_FLAG, DAEMON_SIDECAR_NAME, DaemonLifecyclePhase,
        classify_daemon_call_error, daemon_executable_name, parse_daemon_status_response,
        resolve_env_daemon_executable_from, should_run_daemon_lifecycle_smoke,
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
    fn daemon_lifecycle_smoke_flag_is_explicit_and_hidden() {
        assert!(should_run_daemon_lifecycle_smoke([OsString::from(
            DAEMON_LIFECYCLE_SMOKE_FLAG
        )]));
        assert!(should_run_daemon_lifecycle_smoke([
            OsString::from("--ordinary-open-argument"),
            OsString::from(DAEMON_LIFECYCLE_SMOKE_FLAG)
        ]));
        assert!(!should_run_daemon_lifecycle_smoke([OsString::from(
            "--smoke-daemon-lifecycle"
        )]));
        assert!(!should_run_daemon_lifecycle_smoke([OsString::from(
            "document.akraz"
        )]));
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
}
