use akraz_ipc::{
    DaemonStatus, DaemonStatusParams, IpcCallError, IpcEndpoint, IpcTransportError,
    JSONRPC_VERSION, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcClient,
    METHOD_DAEMON_STATUS, OsLocalIpcClient, call_json_rpc, resolve_current_default_endpoint,
};

const LOCAL_REQUEST_ID: &str = "tauri";

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![daemon_status])
        .run(tauri::generate_context!())
}

#[tauri::command]
async fn daemon_status() -> Result<DaemonStatus, String> {
    tauri::async_runtime::spawn_blocking(call_daemon_status)
        .await
        .map_err(|error| format!("daemon status task failed: {error}"))?
}

fn call_daemon_status() -> Result<DaemonStatus, String> {
    let endpoint = resolve_current_default_endpoint().map_err(|error| error.to_string())?;
    let client = OsLocalIpcClient::new(endpoint);
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    );
    let response_line = call_json_rpc(&client, &request)
        .map_err(|error| format_daemon_call_error(&error, client.endpoint()))?;

    parse_daemon_status_response(&response_line)
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

fn format_daemon_call_error(error: &IpcCallError, fallback_endpoint: &IpcEndpoint) -> String {
    match error {
        IpcCallError::Transport {
            source: IpcTransportError::EndpointUnavailable { endpoint, message },
        } => format!(
            "akraz daemon is not reachable at {endpoint}. Start akraz-daemon, then refresh this screen. Details: {message}"
        ),
        IpcCallError::Transport {
            source: IpcTransportError::RequestFailed { message },
        } => format!("akraz daemon IPC request failed at {fallback_endpoint}. Details: {message}"),
        IpcCallError::Encode { source } => {
            format!("failed to encode daemon IPC request: {source}")
        }
    }
}

#[cfg(test)]
mod tests {
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, IpcEndpoint, IpcPlatformCapabilities, IpcTransportError,
        JsonRpcError, JsonRpcFailure, JsonRpcSuccess, ProtocolVersionSnapshot, to_json_line,
    };

    use super::{format_daemon_call_error, parse_daemon_status_response};

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
    fn daemon_call_error_mentions_refresh_recovery() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = akraz_ipc::IpcCallError::Transport {
            source: IpcTransportError::endpoint_unavailable(endpoint.clone(), "not found"),
        };

        assert_eq!(
            format_daemon_call_error(&error, &endpoint),
            "akraz daemon is not reachable at local-test. Start akraz-daemon, then refresh this screen. Details: not found"
        );
    }
}
