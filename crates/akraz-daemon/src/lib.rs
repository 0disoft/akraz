//! Daemon status builders shared by akraz daemon and diagnostic clients.

use std::error::Error;
use std::fmt::{Display, Formatter};

use akraz_core::RuntimeInputState;
use akraz_ipc::{
    DaemonStatus, IpcCodecError, IpcPlatformCapabilities, IpcRequest, IpcTransportError,
    JsonRpcError, JsonRpcFailure, JsonRpcSuccess, LocalIpcServer, PermissionIssue,
    PermissionsProbe, ProtocolVersionSnapshot, parse_request_line, serve_os_local_ipc_once,
    to_json_line,
};
use akraz_platform::{PlatformAdapter, PlatformError};
use akraz_protocol::ProtocolVersion;

/// Current daemon package version.
pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

const JSONRPC_DAEMON_ERROR: i32 = -32000;

/// Daemon OS IPC serving configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonIpcRunConfig {
    endpoint: akraz_ipc::IpcEndpoint,
    max_requests: Option<usize>,
}

impl DaemonIpcRunConfig {
    /// Serve indefinitely at the selected endpoint.
    pub fn serve_forever(endpoint: akraz_ipc::IpcEndpoint) -> Self {
        Self {
            endpoint,
            max_requests: None,
        }
    }

    /// Serve a bounded number of requests at the selected endpoint.
    pub fn serve_requests(endpoint: akraz_ipc::IpcEndpoint, max_requests: usize) -> Self {
        Self {
            endpoint,
            max_requests: Some(max_requests),
        }
    }

    /// The OS IPC endpoint this daemon run will bind.
    pub fn endpoint(&self) -> &akraz_ipc::IpcEndpoint {
        &self.endpoint
    }
}

/// In-process local IPC server backed by daemon runtime state.
#[derive(Debug)]
pub struct DaemonIpcServer<P> {
    state: RuntimeInputState,
    platform: P,
}

impl<P> DaemonIpcServer<P> {
    /// Create an in-process daemon IPC server.
    pub fn new(state: RuntimeInputState, platform: P) -> Self {
        Self { state, platform }
    }
}

impl<P> LocalIpcServer for DaemonIpcServer<P>
where
    P: PlatformAdapter,
{
    fn handle_request_line(&self, request_line: &str) -> Result<String, IpcTransportError> {
        handle_ipc_request_line(&self.state, &self.platform, request_line)
            .map_err(|error| IpcTransportError::request_failed(error.to_string()))
    }
}

/// Serve daemon IPC requests on an OS-backed endpoint.
pub fn serve_daemon_ipc<P>(
    config: &DaemonIpcRunConfig,
    server: &DaemonIpcServer<P>,
) -> Result<(), IpcTransportError>
where
    P: PlatformAdapter,
{
    let mut handled_requests = 0usize;

    loop {
        if config
            .max_requests
            .is_some_and(|max_requests| handled_requests >= max_requests)
        {
            return Ok(());
        }

        serve_os_local_ipc_once(config.endpoint(), server)?;
        handled_requests += 1;
    }
}

/// Error returned while encoding a daemon IPC response.
#[derive(Debug)]
pub struct DaemonIpcError {
    source: IpcCodecError,
}

impl DaemonIpcError {
    fn from_source(source: IpcCodecError) -> Self {
        Self { source }
    }
}

impl Display for DaemonIpcError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to encode daemon IPC response: {}",
            self.source
        )
    }
}

impl Error for DaemonIpcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// Build a `daemon.status` result from the current runtime state and platform adapter.
pub fn build_daemon_status(
    state: &RuntimeInputState,
    platform: &impl PlatformAdapter,
) -> Result<DaemonStatus, PlatformError> {
    let capabilities = platform.probe_capabilities()?;

    Ok(DaemonStatus {
        daemon_version: DAEMON_VERSION.to_string(),
        mode: state.mode().into(),
        protocol: ProtocolVersionSnapshot::from(ProtocolVersion::CURRENT),
        peers: Vec::new(),
        capabilities: IpcPlatformCapabilities::from(capabilities),
    })
}

/// Handle one local IPC JSON-RPC request line.
pub fn handle_ipc_request_line(
    state: &RuntimeInputState,
    platform: &impl PlatformAdapter,
    line: &str,
) -> Result<String, DaemonIpcError> {
    match parse_request_line(line) {
        Ok(request) => handle_ipc_request(state, platform, request),
        Err(failure) => encode_response(&failure),
    }
}

fn handle_ipc_request(
    state: &RuntimeInputState,
    platform: &impl PlatformAdapter,
    request: IpcRequest,
) -> Result<String, DaemonIpcError> {
    match request {
        IpcRequest::DaemonStatus(request) => match build_daemon_status(state, platform) {
            Ok(status) => encode_response(&JsonRpcSuccess::new(request.id, status)),
            Err(error) => encode_platform_error(request.id, "daemon status unavailable", error),
        },
        IpcRequest::PermissionsProbe(request) => match build_permissions_probe(platform) {
            Ok(probe) => encode_response(&JsonRpcSuccess::new(request.id, probe)),
            Err(error) => encode_platform_error(request.id, "permissions probe unavailable", error),
        },
    }
}

fn encode_platform_error(
    id: String,
    message: &'static str,
    error: PlatformError,
) -> Result<String, DaemonIpcError> {
    encode_response(&JsonRpcFailure::new(
        Some(id),
        JsonRpcError::new(JSONRPC_DAEMON_ERROR, format!("{message}: {error}")),
    ))
}

fn encode_response<T>(response: &T) -> Result<String, DaemonIpcError>
where
    T: serde::Serialize,
{
    to_json_line(response).map_err(DaemonIpcError::from_source)
}

/// Build a `permissions.probe` result from the selected platform adapter.
pub fn build_permissions_probe(
    platform: &impl PlatformAdapter,
) -> Result<PermissionsProbe, PlatformError> {
    let capabilities = platform.probe_capabilities()?;
    let mut issues = Vec::new();

    push_missing_capability_issue(
        &mut issues,
        capabilities.can_capture_pointer,
        "capture_pointer_unavailable",
        "Pointer capture is not available.",
    );
    push_missing_capability_issue(
        &mut issues,
        capabilities.can_capture_keyboard,
        "capture_keyboard_unavailable",
        "Keyboard capture is not available.",
    );
    push_missing_capability_issue(
        &mut issues,
        capabilities.can_inject_pointer,
        "inject_pointer_unavailable",
        "Pointer injection is not available.",
    );
    push_missing_capability_issue(
        &mut issues,
        capabilities.can_inject_keyboard,
        "inject_keyboard_unavailable",
        "Keyboard injection is not available.",
    );

    Ok(PermissionsProbe {
        adapter_name: platform.name().to_string(),
        capabilities: IpcPlatformCapabilities::from(capabilities),
        issues,
    })
}

fn push_missing_capability_issue(
    issues: &mut Vec<PermissionIssue>,
    available: bool,
    code: &'static str,
    message: &'static str,
) {
    if !available {
        issues.push(PermissionIssue {
            code: code.to_string(),
            message: message.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use akraz_core::{ControlMode, RuntimeInputState};
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, DaemonStatusParams, IpcEndpoint,
        IpcPlatformCapabilities, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcServer,
        METHOD_DAEMON_STATUS, OsLocalIpcClient, call_json_rpc, to_json_line,
    };
    use akraz_platform::{FakePlatformAdapter, PlatformCapabilities};
    use serde_json::json;

    use super::{
        DAEMON_VERSION, DaemonIpcRunConfig, DaemonIpcServer, build_daemon_status,
        build_permissions_probe, handle_ipc_request_line, serve_daemon_ipc,
    };

    fn status_or_panic(
        state: &RuntimeInputState,
        platform: &FakePlatformAdapter,
    ) -> akraz_ipc::DaemonStatus {
        match build_daemon_status(state, platform) {
            Ok(status) => status,
            Err(error) => panic!("expected daemon status: {error}"),
        }
    }

    fn probe_or_panic(platform: &FakePlatformAdapter) -> akraz_ipc::PermissionsProbe {
        match build_permissions_probe(platform) {
            Ok(probe) => probe,
            Err(error) => panic!("expected permission probe: {error}"),
        }
    }

    #[cfg(unix)]
    fn unique_os_endpoint() -> IpcEndpoint {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "akraz-daemon-test-{}-{nanos}.sock",
            std::process::id()
        ));

        match IpcEndpoint::unix_socket(path.to_string_lossy().into_owned()) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected Unix socket endpoint: {error}"),
        }
    }

    #[cfg(windows)]
    fn unique_os_endpoint() -> IpcEndpoint {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);

        match IpcEndpoint::manual(format!(
            r"\\.\pipe\akraz-daemon-test-{}-{nanos}",
            std::process::id()
        )) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected Windows named pipe endpoint: {error}"),
        }
    }

    #[cfg(any(unix, windows))]
    fn call_with_short_retry(
        client: &OsLocalIpcClient,
        request: &JsonRpcRequest<DaemonStatusParams>,
    ) -> Result<String, String> {
        let mut last_error = None;
        for _ in 0..20 {
            match call_json_rpc(client, request) {
                Ok(response) => return Ok(response),
                Err(error @ akraz_ipc::IpcCallError::Transport { .. }) => {
                    last_error = Some(error.to_string());
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => return Err(error.to_string()),
            }
        }

        Err(last_error.unwrap_or_else(|| "retry exhausted".to_string()))
    }

    #[test]
    fn daemon_status_reflects_runtime_state_and_capabilities() {
        let state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();

        let status = status_or_panic(&state, &platform);

        assert_eq!(status.daemon_version, DAEMON_VERSION);
        assert_eq!(status.mode, ControlModeSnapshot::from(ControlMode::Local));
        assert_eq!(status.protocol.major, 1);
        assert_eq!(status.protocol.minor, 0);
        assert!(status.peers.is_empty());
        assert_eq!(
            status.capabilities,
            IpcPlatformCapabilities {
                can_capture_pointer: true,
                can_capture_keyboard: true,
                can_inject_pointer: true,
                can_inject_keyboard: true,
            }
        );
    }

    #[test]
    fn permissions_probe_reports_missing_capability_issues() {
        let platform = FakePlatformAdapter::new(PlatformCapabilities {
            can_capture_pointer: true,
            can_capture_keyboard: false,
            can_inject_pointer: true,
            can_inject_keyboard: false,
        });

        let probe = probe_or_panic(&platform);

        assert_eq!(probe.adapter_name, "fake");
        assert_eq!(probe.issues.len(), 2);
        assert_eq!(probe.issues[0].code, "capture_keyboard_unavailable");
        assert_eq!(probe.issues[1].code, "inject_keyboard_unavailable");
    }

    #[test]
    fn ipc_dispatch_handles_daemon_status_request() {
        let state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let request =
            JsonRpcRequest::new("req_1", METHOD_DAEMON_STATUS, DaemonStatusParams::default());
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line = match handle_ipc_request_line(&state, &platform, &request_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon IPC response: {error}"),
        };
        let response: JsonRpcSuccess<DaemonStatus> = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected daemon status response JSON: {error}"),
        };

        assert_eq!(response.id, "req_1");
        assert_eq!(response.result.daemon_version, DAEMON_VERSION);
        assert_eq!(response.result.mode, ControlModeSnapshot::Local);
    }

    #[test]
    fn daemon_ipc_server_implements_local_server_contract() {
        let server = DaemonIpcServer::new(RuntimeInputState::new(), FakePlatformAdapter::default());
        let request =
            JsonRpcRequest::new("req_1", METHOD_DAEMON_STATUS, DaemonStatusParams::default());
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line = match server.handle_request_line(&request_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon server response: {error}"),
        };
        let response: JsonRpcSuccess<DaemonStatus> = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected daemon status response JSON: {error}"),
        };

        assert_eq!(response.id, "req_1");
        assert_eq!(response.result.daemon_version, DAEMON_VERSION);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn daemon_ipc_loop_serves_bounded_os_requests() {
        let endpoint = unique_os_endpoint();
        let server_endpoint = endpoint.clone();
        let server_thread = std::thread::spawn(move || {
            let server =
                DaemonIpcServer::new(RuntimeInputState::new(), FakePlatformAdapter::default());
            let config = DaemonIpcRunConfig::serve_requests(server_endpoint, 1);

            serve_daemon_ipc(&config, &server)
        });
        let client = OsLocalIpcClient::new(endpoint);
        let request =
            JsonRpcRequest::new("req_1", METHOD_DAEMON_STATUS, DaemonStatusParams::default());

        let response_line = match call_with_short_retry(&client, &request) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon IPC response: {error}"),
        };
        let server_result = match server_thread.join() {
            Ok(result) => result,
            Err(_) => panic!("expected daemon IPC server thread to finish"),
        };
        let response: JsonRpcSuccess<DaemonStatus> = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected daemon status response JSON: {error}"),
        };

        assert_eq!(server_result, Ok(()));
        assert_eq!(response.id, "req_1");
        assert_eq!(response.result.daemon_version, DAEMON_VERSION);
    }

    #[test]
    fn ipc_dispatch_handles_unknown_method_failure() {
        let state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let request_line = r#"{"jsonrpc":"2.0","id":"req_1","method":"daemon.nope","params":{}}"#;

        let response_line = match handle_ipc_request_line(&state, &platform, request_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon IPC failure response: {error}"),
        };
        let response: JsonRpcFailure = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected JSON-RPC failure response: {error}"),
        };

        assert_eq!(response.id, Some("req_1".to_string()));
        assert_eq!(
            response.error.code,
            akraz_ipc::JSONRPC_ERROR_METHOD_NOT_FOUND
        );
        assert_eq!(response.error.message, "method not found: daemon.nope");
    }

    #[test]
    fn ipc_dispatch_handles_malformed_request_failure() {
        let state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();

        let response_line = match handle_ipc_request_line(&state, &platform, "{not json") {
            Ok(line) => line,
            Err(error) => panic!("expected daemon IPC parse failure response: {error}"),
        };
        let value: serde_json::Value = match serde_json::from_str(&response_line) {
            Ok(value) => value,
            Err(error) => panic!("expected JSON-RPC failure response: {error}"),
        };

        assert_eq!(
            value,
            json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {
                    "code": akraz_ipc::JSONRPC_ERROR_PARSE,
                    "message": "parse error"
                }
            })
        );
    }
}
