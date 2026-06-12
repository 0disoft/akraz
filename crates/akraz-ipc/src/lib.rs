//! Local IPC JSON-RPC contract shared by akraz daemon, CLI, and UI callers.

use std::error::Error;
use std::fmt::{Display, Formatter};

use akraz_core::ControlMode;
use akraz_platform::PlatformCapabilities;
use akraz_protocol::ProtocolVersion;
use serde::{Deserialize, Serialize};

/// JSON-RPC protocol marker used by akraz local IPC.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC method for daemon status.
pub const METHOD_DAEMON_STATUS: &str = "daemon.status";

/// JSON-RPC method for platform permission probing.
pub const METHOD_PERMISSIONS_PROBE: &str = "permissions.probe";

/// JSON-RPC request envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcRequest<P> {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: P,
}

impl<P> JsonRpcRequest<P> {
    /// Create a request envelope.
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC success response envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcSuccess<T> {
    pub jsonrpc: String,
    pub id: String,
    pub result: T,
}

impl<T> JsonRpcSuccess<T> {
    /// Create a success response envelope.
    pub fn new(id: impl Into<String>, result: T) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            result,
        }
    }
}

/// JSON-RPC error response envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcFailure {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub error: JsonRpcError,
}

impl JsonRpcFailure {
    /// Create an error response envelope.
    pub fn new(id: Option<String>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            error,
        }
    }
}

/// JSON-RPC error payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcError {
    /// Create an error payload.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Empty params for `daemon.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusParams {}

/// Empty params for `permissions.probe`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsProbeParams {}

/// Wire-safe snapshot of the core control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlModeSnapshot {
    Local,
    EnteringRemote,
    Remote,
    LeavingRemote,
    Suspended,
}

impl From<ControlMode> for ControlModeSnapshot {
    fn from(mode: ControlMode) -> Self {
        match mode {
            ControlMode::Local => Self::Local,
            ControlMode::EnteringRemote => Self::EnteringRemote,
            ControlMode::Remote => Self::Remote,
            ControlMode::LeavingRemote => Self::LeavingRemote,
            ControlMode::Suspended => Self::Suspended,
        }
    }
}

/// Wire-safe protocol version snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersionSnapshot {
    pub major: u16,
    pub minor: u16,
}

impl From<ProtocolVersion> for ProtocolVersionSnapshot {
    fn from(version: ProtocolVersion) -> Self {
        Self {
            major: version.major,
            minor: version.minor,
        }
    }
}

/// Wire-safe platform capability snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcPlatformCapabilities {
    pub can_capture_pointer: bool,
    pub can_capture_keyboard: bool,
    pub can_inject_pointer: bool,
    pub can_inject_keyboard: bool,
}

impl From<PlatformCapabilities> for IpcPlatformCapabilities {
    fn from(capabilities: PlatformCapabilities) -> Self {
        Self {
            can_capture_pointer: capabilities.can_capture_pointer,
            can_capture_keyboard: capabilities.can_capture_keyboard,
            can_inject_pointer: capabilities.can_inject_pointer,
            can_inject_keyboard: capabilities.can_inject_keyboard,
        }
    }
}

/// Minimal peer status placeholder for the first status contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    pub peer_id: String,
    pub display_name: String,
    pub connected: bool,
}

/// `daemon.status` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub daemon_version: String,
    pub mode: ControlModeSnapshot,
    pub protocol: ProtocolVersionSnapshot,
    pub peers: Vec<PeerStatus>,
    pub capabilities: IpcPlatformCapabilities,
}

/// `permissions.probe` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsProbe {
    pub adapter_name: String,
    pub capabilities: IpcPlatformCapabilities,
    pub issues: Vec<PermissionIssue>,
}

/// Platform permission issue returned by `permissions.probe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionIssue {
    pub code: String,
    pub message: String,
}

/// IPC JSON codec error.
#[derive(Debug)]
pub struct IpcCodecError {
    source: serde_json::Error,
}

impl IpcCodecError {
    fn from_source(source: serde_json::Error) -> Self {
        Self { source }
    }
}

impl Display for IpcCodecError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "failed to encode IPC JSON: {}", self.source)
    }
}

impl Error for IpcCodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// Serialize an IPC value as a single JSON line payload.
pub fn to_json_line<T>(value: &T) -> Result<String, IpcCodecError>
where
    T: Serialize,
{
    serde_json::to_string(value)
        .map(|json| format!("{json}\n"))
        .map_err(IpcCodecError::from_source)
}

#[cfg(test)]
mod tests {
    use super::{
        ControlModeSnapshot, DaemonStatus, IpcPlatformCapabilities, JsonRpcRequest, JsonRpcSuccess,
        METHOD_DAEMON_STATUS, ProtocolVersionSnapshot, to_json_line,
    };
    use serde_json::json;

    fn json_value_or_panic(line: &str) -> serde_json::Value {
        match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => panic!("expected valid JSON: {error}"),
        }
    }

    #[test]
    fn daemon_status_request_uses_json_rpc_envelope() {
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_DAEMON_STATUS,
            super::DaemonStatusParams::default(),
        );
        let line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        assert_eq!(
            json_value_or_panic(&line),
            json!({
                "jsonrpc": "2.0",
                "id": "req_1",
                "method": "daemon.status",
                "params": {}
            })
        );
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn daemon_status_response_uses_camel_case_contract() {
        let status = DaemonStatus {
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
        };
        let response = JsonRpcSuccess::new("req_1", status);
        let line = match to_json_line(&response) {
            Ok(line) => line,
            Err(error) => panic!("expected response serialization: {error}"),
        };

        assert_eq!(
            json_value_or_panic(&line),
            json!({
                "jsonrpc": "2.0",
                "id": "req_1",
                "result": {
                    "daemonVersion": "0.1.0",
                    "mode": "Local",
                    "protocol": {
                        "major": 1,
                        "minor": 0
                    },
                    "peers": [],
                    "capabilities": {
                        "canCapturePointer": true,
                        "canCaptureKeyboard": true,
                        "canInjectPointer": true,
                        "canInjectKeyboard": true
                    }
                }
            })
        );
    }
}
