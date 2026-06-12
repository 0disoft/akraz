use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use akraz_ipc::{
    DaemonStatus, DaemonStatusParams, IpcCallError, IpcEndpoint, IpcTransportError,
    JSONRPC_VERSION, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcClient,
    METHOD_DAEMON_STATUS, OsLocalIpcClient, call_json_rpc, resolve_current_default_endpoint,
};
use serde::Serialize;

const LOCAL_REQUEST_ID: &str = "tauri";
const DAEMON_PATH_ENV: &str = "AKRAZ_DAEMON_PATH";
const DAEMON_START_RETRIES: usize = 50;
const DAEMON_START_RETRY_DELAY: Duration = Duration::from_millis(40);

type ManagedDaemon = Arc<Mutex<DaemonProcessState>>;

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(DaemonProcessState::default())))
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_start,
            daemon_stop
        ])
        .run(tauri::generate_context!())
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
    managed: tauri::State<'_, ManagedDaemon>,
) -> Result<DaemonLifecycleSnapshot, String> {
    let managed = Arc::clone(managed.inner());
    tauri::async_runtime::spawn_blocking(move || start_daemon(&managed))
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
    child: Option<Child>,
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

fn start_daemon(managed: &ManagedDaemon) -> Result<DaemonLifecycleSnapshot, String> {
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

    let executable = match resolve_daemon_executable() {
        Ok(path) => path,
        Err(error) => return Ok(DaemonLifecycleSnapshot::failed(error)),
    };
    let child = match spawn_daemon_process(&executable) {
        Ok(child) => child,
        Err(error) => {
            return Ok(DaemonLifecycleSnapshot::failed(format!(
                "failed to start akraz-daemon at {}: {error}",
                executable.display()
            )));
        }
    };
    let pid = store_managed_child(managed, child)?;

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
    let Some(mut child) = take_managed_child(managed)? else {
        return Ok(read_daemon_snapshot()
            .with_detail("This app did not start the current Akraz background process."));
    };

    if let Err(error) = child.kill() {
        return Ok(DaemonLifecycleSnapshot::failed(format!(
            "failed to stop akraz-daemon: {error}"
        )));
    }
    if let Err(error) = child.wait() {
        return Ok(DaemonLifecycleSnapshot::failed(format!(
            "akraz-daemon stopped, but process cleanup failed: {error}"
        )));
    }

    Ok(read_daemon_snapshot().with_detail("Akraz stopped."))
}

fn read_daemon_snapshot() -> DaemonLifecycleSnapshot {
    match call_daemon_status() {
        Ok(status) => DaemonLifecycleSnapshot::running(status, None),
        Err(snapshot) => snapshot,
    }
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

fn resolve_daemon_executable() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(DAEMON_PATH_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

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

fn spawn_daemon_process(executable: &PathBuf) -> std::io::Result<Child> {
    Command::new(executable)
        .arg("--serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn managed_daemon_pid(managed: &ManagedDaemon) -> Result<Option<u32>, String> {
    let mut state = lock_managed_daemon(managed)?;
    let mut clear_child = false;
    let pid = match state.child.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_status)) => {
                clear_child = true;
                None
            }
            Ok(None) => Some(child.id()),
            Err(_error) => {
                clear_child = true;
                None
            }
        },
        None => None,
    };

    if clear_child {
        state.child = None;
    }

    Ok(pid)
}

fn store_managed_child(managed: &ManagedDaemon, child: Child) -> Result<u32, String> {
    let mut state = lock_managed_daemon(managed)?;
    let mut child = child;
    let pid = child.id();
    let replace_child = match state.child.as_mut() {
        Some(existing) => match existing.try_wait() {
            Ok(Some(_status)) => true,
            Ok(None) => {
                if let Err(error) = child.kill() {
                    return Err(format!(
                        "failed to clean up duplicate akraz-daemon process: {error}"
                    ));
                }
                let _ = child.wait();
                return Ok(existing.id());
            }
            Err(_error) => true,
        },
        None => true,
    };

    if replace_child {
        state.child = Some(child);
    }

    Ok(pid)
}

fn take_managed_child(managed: &ManagedDaemon) -> Result<Option<Child>, String> {
    let mut state = lock_managed_daemon(managed)?;
    let Some(mut child) = state.child.take() else {
        return Ok(None);
    };

    match child.try_wait() {
        Ok(Some(_status)) => Ok(None),
        Ok(None) => Ok(Some(child)),
        Err(_error) => Ok(None),
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
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, IpcEndpoint, IpcPlatformCapabilities, IpcTransportError,
        JsonRpcError, JsonRpcFailure, JsonRpcSuccess, ProtocolVersionSnapshot, to_json_line,
    };

    use super::{
        DaemonLifecyclePhase, classify_daemon_call_error, daemon_executable_name,
        parse_daemon_status_response,
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
}
