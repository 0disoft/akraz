//! Local IPC JSON-RPC contract shared by akraz daemon, CLI, and UI callers.

use std::env;
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

/// Supported local IPC endpoint kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcEndpointKind {
    WindowsNamedPipe,
    UnixSocket,
    Manual,
}

/// Supported operating systems for default endpoint resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcOperatingSystem {
    Windows,
    Linux,
    Macos,
}

/// Local IPC endpoint address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcEndpoint {
    pub kind: IpcEndpointKind,
    pub address: String,
}

impl IpcEndpoint {
    /// Create a Windows named pipe endpoint.
    pub fn windows_named_pipe(user_id: impl AsRef<str>) -> Result<Self, IpcEndpointError> {
        let user_id = require_non_empty("user_id", user_id.as_ref())?;

        Ok(Self {
            kind: IpcEndpointKind::WindowsNamedPipe,
            address: format!(r"\\.\pipe\akrazd-{user_id}"),
        })
    }

    /// Create a Unix domain socket endpoint.
    pub fn unix_socket(path: impl Into<String>) -> Result<Self, IpcEndpointError> {
        let path = path.into();
        require_non_empty("path", &path)?;

        Ok(Self {
            kind: IpcEndpointKind::UnixSocket,
            address: path,
        })
    }

    /// Create an explicitly supplied endpoint address.
    pub fn manual(address: impl Into<String>) -> Result<Self, IpcEndpointError> {
        let address = address.into();
        require_non_empty("address", &address)?;

        Ok(Self {
            kind: IpcEndpointKind::Manual,
            address,
        })
    }
}

impl Display for IpcEndpoint {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.address)
    }
}

/// Environment facts used to resolve the default local IPC endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IpcEndpointEnvironment {
    pub operating_system: Option<IpcOperatingSystem>,
    pub user_id: Option<String>,
    pub xdg_runtime_dir: Option<String>,
    pub home_dir: Option<String>,
}

/// Endpoint resolution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcEndpointError {
    UnsupportedOperatingSystem,
    MissingUserId,
    MissingXdgRuntimeDir,
    MissingHomeDir,
    EmptyValue { field: &'static str },
}

impl Display for IpcEndpointError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedOperatingSystem => {
                formatter.write_str("unsupported operating system for local IPC")
            }
            Self::MissingUserId => formatter.write_str("missing user id for Windows named pipe"),
            Self::MissingXdgRuntimeDir => {
                formatter.write_str("missing XDG_RUNTIME_DIR for Linux Unix socket")
            }
            Self::MissingHomeDir => {
                formatter.write_str("missing home directory for macOS Unix socket")
            }
            Self::EmptyValue { field } => write!(formatter, "empty endpoint {field}"),
        }
    }
}

impl Error for IpcEndpointError {}

/// Resolve the default local IPC endpoint for the current process environment.
pub fn resolve_current_default_endpoint() -> Result<IpcEndpoint, IpcEndpointError> {
    resolve_default_endpoint(&current_endpoint_environment())
}

/// Resolve the default local IPC endpoint from supplied environment facts.
pub fn resolve_default_endpoint(
    environment: &IpcEndpointEnvironment,
) -> Result<IpcEndpoint, IpcEndpointError> {
    match environment.operating_system {
        Some(IpcOperatingSystem::Windows) => {
            let user_id = environment
                .user_id
                .as_deref()
                .ok_or(IpcEndpointError::MissingUserId)?;

            IpcEndpoint::windows_named_pipe(user_id)
        }
        Some(IpcOperatingSystem::Linux) => {
            let runtime_dir = environment
                .xdg_runtime_dir
                .as_deref()
                .ok_or(IpcEndpointError::MissingXdgRuntimeDir)?;

            IpcEndpoint::unix_socket(join_unix_path(runtime_dir, &["akraz", "akrazd.sock"]))
        }
        Some(IpcOperatingSystem::Macos) => {
            let home_dir = environment
                .home_dir
                .as_deref()
                .ok_or(IpcEndpointError::MissingHomeDir)?;

            IpcEndpoint::unix_socket(join_unix_path(
                home_dir,
                &["Library", "Application Support", "Akraz", "akrazd.sock"],
            ))
        }
        None => Err(IpcEndpointError::UnsupportedOperatingSystem),
    }
}

fn current_endpoint_environment() -> IpcEndpointEnvironment {
    IpcEndpointEnvironment {
        operating_system: current_operating_system(),
        user_id: env::var("AKRAZ_USER_ID")
            .ok()
            .or_else(|| env::var("USERNAME").ok())
            .or_else(|| env::var("USER").ok()),
        xdg_runtime_dir: env::var("XDG_RUNTIME_DIR").ok(),
        home_dir: env::var("HOME")
            .ok()
            .or_else(|| env::var("USERPROFILE").ok()),
    }
}

fn current_operating_system() -> Option<IpcOperatingSystem> {
    if cfg!(target_os = "windows") {
        Some(IpcOperatingSystem::Windows)
    } else if cfg!(target_os = "linux") {
        Some(IpcOperatingSystem::Linux)
    } else if cfg!(target_os = "macos") {
        Some(IpcOperatingSystem::Macos)
    } else {
        None
    }
}

fn join_unix_path(root: &str, segments: &[&str]) -> String {
    let mut path = root.trim_end_matches('/').to_string();
    for segment in segments {
        path.push('/');
        path.push_str(segment.trim_matches('/'));
    }
    path
}

fn require_non_empty<'a>(field: &'static str, value: &'a str) -> Result<&'a str, IpcEndpointError> {
    let value = value.trim();
    if value.is_empty() {
        Err(IpcEndpointError::EmptyValue { field })
    } else {
        Ok(value)
    }
}

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
        ControlModeSnapshot, DaemonStatus, IpcEndpoint, IpcEndpointEnvironment, IpcEndpointError,
        IpcEndpointKind, IpcOperatingSystem, IpcPlatformCapabilities, JsonRpcRequest,
        JsonRpcSuccess, METHOD_DAEMON_STATUS, ProtocolVersionSnapshot, resolve_default_endpoint,
        to_json_line,
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

    #[test]
    fn resolves_windows_named_pipe_endpoint() {
        let endpoint = resolve_default_endpoint(&IpcEndpointEnvironment {
            operating_system: Some(IpcOperatingSystem::Windows),
            user_id: Some("S-1-5-21-1000".to_string()),
            ..Default::default()
        });

        assert_eq!(
            endpoint,
            Ok(IpcEndpoint {
                kind: IpcEndpointKind::WindowsNamedPipe,
                address: r"\\.\pipe\akrazd-S-1-5-21-1000".to_string(),
            })
        );
    }

    #[test]
    fn resolves_linux_unix_socket_endpoint() {
        let endpoint = resolve_default_endpoint(&IpcEndpointEnvironment {
            operating_system: Some(IpcOperatingSystem::Linux),
            xdg_runtime_dir: Some("/run/user/1000".to_string()),
            ..Default::default()
        });

        assert_eq!(
            endpoint,
            Ok(IpcEndpoint {
                kind: IpcEndpointKind::UnixSocket,
                address: "/run/user/1000/akraz/akrazd.sock".to_string(),
            })
        );
    }

    #[test]
    fn resolves_macos_unix_socket_endpoint() {
        let endpoint = resolve_default_endpoint(&IpcEndpointEnvironment {
            operating_system: Some(IpcOperatingSystem::Macos),
            home_dir: Some("/Users/cherry".to_string()),
            ..Default::default()
        });

        assert_eq!(
            endpoint,
            Ok(IpcEndpoint {
                kind: IpcEndpointKind::UnixSocket,
                address: "/Users/cherry/Library/Application Support/Akraz/akrazd.sock".to_string(),
            })
        );
    }

    #[test]
    fn endpoint_resolution_reports_missing_required_facts() {
        assert_eq!(
            resolve_default_endpoint(&IpcEndpointEnvironment {
                operating_system: Some(IpcOperatingSystem::Windows),
                ..Default::default()
            }),
            Err(IpcEndpointError::MissingUserId)
        );
        assert_eq!(
            resolve_default_endpoint(&IpcEndpointEnvironment {
                operating_system: Some(IpcOperatingSystem::Linux),
                ..Default::default()
            }),
            Err(IpcEndpointError::MissingXdgRuntimeDir)
        );
        assert_eq!(
            resolve_default_endpoint(&IpcEndpointEnvironment {
                operating_system: Some(IpcOperatingSystem::Macos),
                ..Default::default()
            }),
            Err(IpcEndpointError::MissingHomeDir)
        );
    }
}
