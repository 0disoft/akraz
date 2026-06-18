//! Local IPC JSON-RPC contract shared by akraz daemon, CLI, and UI callers.

use std::env;
use std::error::Error;
#[cfg(windows)]
use std::ffi::OsStr;
use std::fmt::{Display, Formatter};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(unix)]
use std::path::Path;
#[cfg(windows)]
use std::ptr::{null, null_mut};

use akraz_core::{ControlMode, LogicalPoint, LogicalRect};
use akraz_platform::PlatformCapabilities;
use akraz_protocol::ProtocolVersion;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

/// JSON-RPC protocol marker used by akraz local IPC.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC method for daemon status.
pub const METHOD_DAEMON_STATUS: &str = "daemon.status";

/// JSON-RPC method for platform permission probing.
pub const METHOD_PERMISSIONS_PROBE: &str = "permissions.probe";

/// JSON-RPC method for sanitized diagnostic screen topology.
pub const METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY: &str = "diagnostics.screenTopology";

/// JSON-RPC method for emergency input release and local-control recovery.
pub const METHOD_INPUT_RELEASE_ALL: &str = "input.releaseAll";

/// JSON-RPC method for connecting the active peer session transport.
pub const METHOD_SESSION_CONNECT: &str = "session.connect";

/// JSON-RPC method for disconnecting the active peer session transport.
pub const METHOD_SESSION_DISCONNECT: &str = "session.disconnect";

/// JSON-RPC parse error code.
pub const JSONRPC_ERROR_PARSE: i32 = -32700;

/// JSON-RPC invalid request error code.
pub const JSONRPC_ERROR_INVALID_REQUEST: i32 = -32600;

/// JSON-RPC method not found error code.
pub const JSONRPC_ERROR_METHOD_NOT_FOUND: i32 = -32601;

/// JSON-RPC invalid params error code.
pub const JSONRPC_ERROR_INVALID_PARAMS: i32 = -32602;

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

/// Local IPC transport failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcTransportError {
    EndpointUnavailable {
        endpoint: IpcEndpoint,
        message: String,
    },
    RequestFailed {
        message: String,
    },
}

impl IpcTransportError {
    /// Build an endpoint-unavailable transport error.
    pub fn endpoint_unavailable(endpoint: IpcEndpoint, message: impl Into<String>) -> Self {
        Self::EndpointUnavailable {
            endpoint,
            message: message.into(),
        }
    }

    /// Build a request-failed transport error.
    pub fn request_failed(message: impl Into<String>) -> Self {
        Self::RequestFailed {
            message: message.into(),
        }
    }
}

impl Display for IpcTransportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EndpointUnavailable { endpoint, message } => {
                write!(
                    formatter,
                    "IPC endpoint unavailable at {endpoint}: {message}"
                )
            }
            Self::RequestFailed { message } => write!(formatter, "IPC request failed: {message}"),
        }
    }
}

impl Error for IpcTransportError {}

/// Local IPC server contract used by OS transports and in-process diagnostics.
pub trait LocalIpcServer {
    fn handle_request_line(&self, request_line: &str) -> Result<String, IpcTransportError>;
}

/// Local IPC client contract used by CLI and UI callers.
pub trait LocalIpcClient {
    fn endpoint(&self) -> &IpcEndpoint;

    fn send_request_line(&self, request_line: &str) -> Result<String, IpcTransportError>;
}

/// In-process local IPC client used until OS pipe/socket transports are attached.
#[derive(Debug, Clone)]
pub struct InProcessIpcClient<S> {
    endpoint: IpcEndpoint,
    server: S,
}

impl<S> InProcessIpcClient<S> {
    /// Create an in-process client backed by a local server implementation.
    pub fn new(endpoint: IpcEndpoint, server: S) -> Self {
        Self { endpoint, server }
    }
}

impl<S> LocalIpcClient for InProcessIpcClient<S>
where
    S: LocalIpcServer,
{
    fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
    }

    fn send_request_line(&self, request_line: &str) -> Result<String, IpcTransportError> {
        self.server.handle_request_line(request_line)
    }
}

/// OS-backed local IPC client.
#[derive(Debug, Clone)]
pub struct OsLocalIpcClient {
    endpoint: IpcEndpoint,
}

impl OsLocalIpcClient {
    /// Create an OS-backed local IPC client.
    pub fn new(endpoint: IpcEndpoint) -> Self {
        Self { endpoint }
    }
}

impl LocalIpcClient for OsLocalIpcClient {
    fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
    }

    fn send_request_line(&self, request_line: &str) -> Result<String, IpcTransportError> {
        send_os_request_line(&self.endpoint, request_line)
    }
}

/// Serve one OS-backed local IPC request and write its response.
pub fn serve_os_local_ipc_once<S>(
    endpoint: &IpcEndpoint,
    server: &S,
) -> Result<(), IpcTransportError>
where
    S: LocalIpcServer,
{
    serve_os_request_once(endpoint, server)
}

#[cfg(unix)]
fn send_os_request_line(
    endpoint: &IpcEndpoint,
    request_line: &str,
) -> Result<String, IpcTransportError> {
    match endpoint.kind {
        IpcEndpointKind::UnixSocket | IpcEndpointKind::Manual => {
            let mut stream = UnixStream::connect(&endpoint.address).map_err(|error| {
                IpcTransportError::endpoint_unavailable(endpoint.clone(), error.to_string())
            })?;
            stream
                .write_all(request_line.as_bytes())
                .map_err(|error| IpcTransportError::request_failed(error.to_string()))?;
            stream
                .flush()
                .map_err(|error| IpcTransportError::request_failed(error.to_string()))?;

            read_response_line(BufReader::new(stream))
        }
        IpcEndpointKind::WindowsNamedPipe => Err(IpcTransportError::endpoint_unavailable(
            endpoint.clone(),
            "Windows named pipes are not available on this platform",
        )),
    }
}

#[cfg(unix)]
fn serve_os_request_once<S>(endpoint: &IpcEndpoint, server: &S) -> Result<(), IpcTransportError>
where
    S: LocalIpcServer,
{
    match endpoint.kind {
        IpcEndpointKind::UnixSocket | IpcEndpointKind::Manual => {
            prepare_unix_socket_path(endpoint)?;
            let listener = UnixListener::bind(&endpoint.address).map_err(|error| {
                IpcTransportError::endpoint_unavailable(endpoint.clone(), error.to_string())
            })?;
            let (mut stream, _) = listener
                .accept()
                .map_err(|error| IpcTransportError::request_failed(error.to_string()))?;
            let request_line = read_request_line(BufReader::new(
                stream
                    .try_clone()
                    .map_err(|error| IpcTransportError::request_failed(error.to_string()))?,
            ))?;
            let response_line = server.handle_request_line(&request_line)?;
            stream
                .write_all(response_line.as_bytes())
                .map_err(|error| IpcTransportError::request_failed(error.to_string()))?;
            stream
                .flush()
                .map_err(|error| IpcTransportError::request_failed(error.to_string()))?;
            cleanup_unix_socket_path(endpoint);

            Ok(())
        }
        IpcEndpointKind::WindowsNamedPipe => Err(IpcTransportError::endpoint_unavailable(
            endpoint.clone(),
            "Windows named pipes are not available on this platform",
        )),
    }
}

#[cfg(unix)]
fn prepare_unix_socket_path(endpoint: &IpcEndpoint) -> Result<(), IpcTransportError> {
    let path = Path::new(&endpoint.address);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            IpcTransportError::endpoint_unavailable(endpoint.clone(), error.to_string())
        })?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|error| {
                IpcTransportError::endpoint_unavailable(endpoint.clone(), error.to_string())
            })
        }
        Ok(_) => Err(IpcTransportError::endpoint_unavailable(
            endpoint.clone(),
            "socket path exists and is not a Unix socket",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(IpcTransportError::endpoint_unavailable(
            endpoint.clone(),
            error.to_string(),
        )),
    }
}

#[cfg(unix)]
fn cleanup_unix_socket_path(endpoint: &IpcEndpoint) {
    let path = Path::new(&endpoint.address);
    let _ = fs::remove_file(path);
}

#[cfg(windows)]
fn send_os_request_line(
    endpoint: &IpcEndpoint,
    request_line: &str,
) -> Result<String, IpcTransportError> {
    match endpoint.kind {
        IpcEndpointKind::WindowsNamedPipe | IpcEndpointKind::Manual => {
            let pipe_name = wide_null(&endpoint.address);
            // SAFETY: `pipe_name` is a null-terminated UTF-16 buffer that remains alive for the
            // duration of the call, and all pointer parameters either point to valid values or null.
            let handle = unsafe {
                CreateFileW(
                    pipe_name.as_ptr(),
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(IpcTransportError::endpoint_unavailable(
                    endpoint.clone(),
                    std::io::Error::last_os_error().to_string(),
                ));
            }

            let handle = OwnedWindowsHandle::new(handle);
            write_windows_pipe(handle.raw(), request_line.as_bytes())?;
            read_windows_pipe_line(handle.raw())
        }
        IpcEndpointKind::UnixSocket => Err(IpcTransportError::endpoint_unavailable(
            endpoint.clone(),
            "Unix sockets are not available on this platform",
        )),
    }
}

#[cfg(windows)]
fn serve_os_request_once<S>(endpoint: &IpcEndpoint, server: &S) -> Result<(), IpcTransportError>
where
    S: LocalIpcServer,
{
    match endpoint.kind {
        IpcEndpointKind::WindowsNamedPipe | IpcEndpointKind::Manual => {
            let pipe_name = wide_null(&endpoint.address);
            // SAFETY: `pipe_name` is a null-terminated UTF-16 buffer that remains alive for the
            // duration of the call, and the security attributes pointer is intentionally null.
            let handle = unsafe {
                CreateNamedPipeW(
                    pipe_name.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    4096,
                    4096,
                    0,
                    null(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(IpcTransportError::endpoint_unavailable(
                    endpoint.clone(),
                    std::io::Error::last_os_error().to_string(),
                ));
            }

            let handle = OwnedWindowsHandle::new(handle);
            // SAFETY: `handle` is a valid named-pipe handle from `CreateNamedPipeW`; no overlapped
            // structure is used because this synchronous one-shot server blocks for one client.
            let connected = unsafe { ConnectNamedPipe(handle.raw(), null_mut()) != 0 };
            if !connected {
                // SAFETY: `GetLastError` reads the current thread's last Win32 error.
                let error = unsafe { GetLastError() };
                if error != ERROR_PIPE_CONNECTED {
                    return Err(IpcTransportError::request_failed(
                        std::io::Error::last_os_error().to_string(),
                    ));
                }
            }

            let request_line = read_windows_pipe_line(handle.raw())?;
            let response_line = server.handle_request_line(&request_line)?;
            write_windows_pipe(handle.raw(), response_line.as_bytes())
        }
        IpcEndpointKind::UnixSocket => Err(IpcTransportError::endpoint_unavailable(
            endpoint.clone(),
            "Unix sockets are not available on this platform",
        )),
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct OwnedWindowsHandle(HANDLE);

#[cfg(windows)]
impl OwnedWindowsHandle {
    fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is owned by this wrapper and is closed exactly once on drop.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain([0]).collect()
}

#[cfg(windows)]
fn write_windows_pipe(handle: HANDLE, bytes: &[u8]) -> Result<(), IpcTransportError> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk = &bytes[offset..];
        let mut written = 0u32;
        // SAFETY: `handle` is a valid pipe handle, `chunk.as_ptr()` is valid for `chunk.len()`
        // bytes, and `written` is a valid out pointer for the synchronous call.
        let ok = unsafe {
            WriteFile(
                handle,
                chunk.as_ptr().cast(),
                chunk.len().try_into().unwrap_or(u32::MAX),
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(IpcTransportError::request_failed(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        if written == 0 {
            return Err(IpcTransportError::request_failed(
                "pipe write made no progress",
            ));
        }
        offset += written as usize;
    }

    Ok(())
}

#[cfg(windows)]
fn read_windows_pipe_line(handle: HANDLE) -> Result<String, IpcTransportError> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let mut read = 0u32;
        // SAFETY: `handle` is a valid pipe handle, `buffer` is writable for its length, and `read`
        // is a valid out pointer for the synchronous call.
        let ok = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(IpcTransportError::request_failed(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        if read == 0 {
            return Err(IpcTransportError::request_failed(
                "pipe closed before a response line was received",
            ));
        }

        let chunk = &buffer[..read as usize];
        bytes.extend_from_slice(chunk);
        if chunk.contains(&b'\n') {
            break;
        }
    }

    String::from_utf8(bytes).map_err(|error| IpcTransportError::request_failed(error.to_string()))
}

#[cfg(not(any(unix, windows)))]
fn send_os_request_line(
    endpoint: &IpcEndpoint,
    _request_line: &str,
) -> Result<String, IpcTransportError> {
    Err(IpcTransportError::endpoint_unavailable(
        endpoint.clone(),
        "OS local IPC is not available on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn serve_os_request_once<S>(endpoint: &IpcEndpoint, _server: &S) -> Result<(), IpcTransportError>
where
    S: LocalIpcServer,
{
    Err(IpcTransportError::endpoint_unavailable(
        endpoint.clone(),
        "OS local IPC is not available on this platform",
    ))
}

#[cfg(unix)]
fn read_request_line<R>(reader: R) -> Result<String, IpcTransportError>
where
    R: BufRead,
{
    read_ipc_line(reader, "request")
}

#[cfg(unix)]
fn read_response_line<R>(reader: R) -> Result<String, IpcTransportError>
where
    R: BufRead,
{
    read_ipc_line(reader, "response")
}

#[cfg(unix)]
fn read_ipc_line<R>(mut reader: R, label: &'static str) -> Result<String, IpcTransportError>
where
    R: BufRead,
{
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|error| IpcTransportError::request_failed(error.to_string()))?;
    if read == 0 {
        Err(IpcTransportError::request_failed(format!(
            "IPC {label} stream closed before a line was received"
        )))
    } else {
        Ok(line)
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

/// Decoded local IPC request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcRequest {
    DaemonStatus(JsonRpcRequest<DaemonStatusParams>),
    PermissionsProbe(JsonRpcRequest<PermissionsProbeParams>),
    DiagnosticsScreenTopology(JsonRpcRequest<DiagnosticsScreenTopologyParams>),
    InputReleaseAll(JsonRpcRequest<InputReleaseAllParams>),
    SessionConnect(JsonRpcRequest<SessionConnectParams>),
    SessionDisconnect(JsonRpcRequest<SessionDisconnectParams>),
}

impl IpcRequest {
    /// Return the request id.
    pub fn id(&self) -> &str {
        match self {
            Self::DaemonStatus(request) => &request.id,
            Self::PermissionsProbe(request) => &request.id,
            Self::DiagnosticsScreenTopology(request) => &request.id,
            Self::InputReleaseAll(request) => &request.id,
            Self::SessionConnect(request) => &request.id,
            Self::SessionDisconnect(request) => &request.id,
        }
    }

    /// Return the request method.
    pub fn method(&self) -> &str {
        match self {
            Self::DaemonStatus(request) => &request.method,
            Self::PermissionsProbe(request) => &request.method,
            Self::DiagnosticsScreenTopology(request) => &request.method,
            Self::InputReleaseAll(request) => &request.method,
            Self::SessionConnect(request) => &request.method,
            Self::SessionDisconnect(request) => &request.method,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawJsonRpcRequest {
    jsonrpc: String,
    id: String,
    method: String,
    #[serde(default = "empty_params_value")]
    params: Value,
}

fn empty_params_value() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Empty params for `daemon.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusParams {}

/// Empty params for `permissions.probe`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsProbeParams {}

/// Empty params for `diagnostics.screenTopology`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsScreenTopologyParams {}

/// Empty params for `input.releaseAll`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputReleaseAllParams {}

/// Params for `session.connect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConnectParams {
    pub peer_id: String,
    pub local_device_id: String,
    pub address: String,
}

/// Empty params for `session.disconnect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDisconnectParams {}

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

/// Wire-safe logical point snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicalPointSnapshot {
    pub x: i32,
    pub y: i32,
}

impl From<LogicalPoint> for LogicalPointSnapshot {
    fn from(point: LogicalPoint) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
}

/// Wire-safe logical rectangle snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicalRectSnapshot {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl From<LogicalRect> for LogicalRectSnapshot {
    fn from(rect: LogicalRect) -> Self {
        Self {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
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

/// Sanitized screen topology facts for diagnostic snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsScreenTopology {
    pub pointer_position: LogicalPointSnapshot,
    pub virtual_screen_bounds: LogicalRectSnapshot,
}

/// `input.releaseAll` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputReleaseAllResult {
    pub released: bool,
    pub mode: ControlModeSnapshot,
}

/// Wire-safe active peer session snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub peer_id: String,
    pub local_device_id: String,
    pub address: String,
    pub connected: bool,
}

/// `session.connect` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConnectResult {
    pub connected: bool,
    pub session: SessionStatus,
}

/// `session.disconnect` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDisconnectResult {
    pub disconnected: bool,
    pub session: Option<SessionStatus>,
    pub mode: ControlModeSnapshot,
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

/// Encode and send one JSON-RPC request over a local IPC client.
pub fn call_json_rpc<C, P>(client: &C, request: &JsonRpcRequest<P>) -> Result<String, IpcCallError>
where
    C: LocalIpcClient,
    P: Serialize,
{
    let request_line = to_json_line(request).map_err(IpcCallError::from_codec)?;
    client
        .send_request_line(&request_line)
        .map_err(IpcCallError::from_transport)
}

/// Local IPC call failure at either encoding or transport boundary.
#[derive(Debug)]
pub enum IpcCallError {
    Encode { source: IpcCodecError },
    Transport { source: IpcTransportError },
}

impl IpcCallError {
    fn from_codec(source: IpcCodecError) -> Self {
        Self::Encode { source }
    }

    fn from_transport(source: IpcTransportError) -> Self {
        Self::Transport { source }
    }
}

impl Display for IpcCallError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode { source } => write!(formatter, "failed to encode IPC request: {source}"),
            Self::Transport { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for IpcCallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode { source } => Some(source),
            Self::Transport { source } => Some(source),
        }
    }
}

/// Parse and classify a single JSON-RPC request line.
pub fn parse_request_line(line: &str) -> Result<IpcRequest, JsonRpcFailure> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            return Err(JsonRpcFailure::new(
                None,
                JsonRpcError::new(JSONRPC_ERROR_PARSE, "parse error"),
            ));
        }
    };
    let id = request_id_from_value(&value);
    let raw: RawJsonRpcRequest = match serde_json::from_value(value) {
        Ok(request) => request,
        Err(_) => {
            return Err(JsonRpcFailure::new(
                id,
                JsonRpcError::new(JSONRPC_ERROR_INVALID_REQUEST, "invalid request"),
            ));
        }
    };

    if raw.jsonrpc != JSONRPC_VERSION {
        return Err(JsonRpcFailure::new(
            Some(raw.id),
            JsonRpcError::new(JSONRPC_ERROR_INVALID_REQUEST, "invalid JSON-RPC version"),
        ));
    }

    match raw.method.as_str() {
        METHOD_DAEMON_STATUS => parse_typed_request(raw).map(IpcRequest::DaemonStatus),
        METHOD_PERMISSIONS_PROBE => parse_typed_request(raw).map(IpcRequest::PermissionsProbe),
        METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY => {
            parse_typed_request(raw).map(IpcRequest::DiagnosticsScreenTopology)
        }
        METHOD_INPUT_RELEASE_ALL => parse_typed_request(raw).map(IpcRequest::InputReleaseAll),
        METHOD_SESSION_CONNECT => parse_typed_request(raw).map(IpcRequest::SessionConnect),
        METHOD_SESSION_DISCONNECT => parse_typed_request(raw).map(IpcRequest::SessionDisconnect),
        _ => Err(JsonRpcFailure::new(
            Some(raw.id),
            JsonRpcError::new(
                JSONRPC_ERROR_METHOD_NOT_FOUND,
                format!("method not found: {}", raw.method),
            ),
        )),
    }
}

fn request_id_from_value(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_typed_request<P>(raw: RawJsonRpcRequest) -> Result<JsonRpcRequest<P>, JsonRpcFailure>
where
    P: for<'de> Deserialize<'de>,
{
    let params = if raw.params.is_object() {
        serde_json::from_value(raw.params)
    } else {
        Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "params must be an object",
        )))
    }
    .map_err(|error| {
        JsonRpcFailure::new(
            Some(raw.id.clone()),
            JsonRpcError::new(
                JSONRPC_ERROR_INVALID_PARAMS,
                format!("invalid params for {}: {error}", raw.method),
            ),
        )
    })?;

    Ok(JsonRpcRequest::new(raw.id, raw.method, params))
}

#[cfg(test)]
mod tests {
    use super::{
        ControlModeSnapshot, DaemonStatus, DaemonStatusParams, DiagnosticsScreenTopology,
        DiagnosticsScreenTopologyParams, InProcessIpcClient, IpcEndpoint, IpcEndpointEnvironment,
        IpcEndpointError, IpcEndpointKind, IpcOperatingSystem, IpcPlatformCapabilities, IpcRequest,
        IpcTransportError, JSONRPC_ERROR_INVALID_PARAMS, JSONRPC_ERROR_METHOD_NOT_FOUND,
        JSONRPC_ERROR_PARSE, JsonRpcRequest, JsonRpcSuccess, LocalIpcClient, LocalIpcServer,
        LogicalPointSnapshot, LogicalRectSnapshot, METHOD_DAEMON_STATUS,
        METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY, METHOD_INPUT_RELEASE_ALL, METHOD_SESSION_CONNECT,
        METHOD_SESSION_DISCONNECT, OsLocalIpcClient, ProtocolVersionSnapshot, SessionConnectParams,
        SessionConnectResult, SessionDisconnectParams, SessionDisconnectResult, SessionStatus,
        call_json_rpc, parse_request_line, resolve_default_endpoint, serve_os_local_ipc_once,
        to_json_line,
    };
    use serde_json::json;

    #[derive(Debug, Clone)]
    struct EchoServer;

    impl LocalIpcServer for EchoServer {
        fn handle_request_line(&self, request_line: &str) -> Result<String, IpcTransportError> {
            Ok(format!("handled:{request_line}"))
        }
    }

    #[derive(Debug, Clone)]
    struct FailingServer;

    impl LocalIpcServer for FailingServer {
        fn handle_request_line(&self, _request_line: &str) -> Result<String, IpcTransportError> {
            Err(IpcTransportError::request_failed("server unavailable"))
        }
    }

    fn json_value_or_panic(line: &str) -> serde_json::Value {
        match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => panic!("expected valid JSON: {error}"),
        }
    }

    #[cfg(unix)]
    fn unique_os_endpoint() -> IpcEndpoint {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "akraz-ipc-test-{}-{nanos}.sock",
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
            r"\\.\pipe\akraz-ipc-test-{}-{nanos}",
            std::process::id()
        )) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected Windows named pipe endpoint: {error}"),
        }
    }

    #[cfg(any(unix, windows))]
    fn send_with_short_retry(
        client: &OsLocalIpcClient,
        request_line: &str,
    ) -> Result<String, IpcTransportError> {
        let mut last_error = None;
        for _ in 0..20 {
            match client.send_request_line(request_line) {
                Ok(response) => return Ok(response),
                Err(error @ IpcTransportError::EndpointUnavailable { .. }) => {
                    last_error = Some(error);
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_error.unwrap_or_else(|| IpcTransportError::request_failed("retry exhausted")))
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
    fn diagnostics_screen_topology_response_uses_camel_case_contract() {
        let topology = DiagnosticsScreenTopology {
            pointer_position: LogicalPointSnapshot { x: 1919, y: 540 },
            virtual_screen_bounds: LogicalRectSnapshot {
                x: -1920,
                y: 0,
                width: 3840,
                height: 1080,
            },
        };
        let response = JsonRpcSuccess::new("req_1", topology);
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
                    "pointerPosition": {
                        "x": 1919,
                        "y": 540
                    },
                    "virtualScreenBounds": {
                        "x": -1920,
                        "y": 0,
                        "width": 3840,
                        "height": 1080
                    }
                }
            })
        );
    }

    #[test]
    fn session_connect_request_uses_camel_case_contract() {
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_SESSION_CONNECT,
            SessionConnectParams {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
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
                "method": "session.connect",
                "params": {
                    "peerId": "linux-laptop",
                    "localDeviceId": "windows-desktop",
                    "address": "127.0.0.1:24888"
                }
            })
        );
    }

    #[test]
    fn session_connect_response_uses_camel_case_contract() {
        let response = JsonRpcSuccess::new(
            "req_1",
            SessionConnectResult {
                connected: true,
                session: SessionStatus {
                    peer_id: "linux-laptop".to_string(),
                    local_device_id: "windows-desktop".to_string(),
                    address: "127.0.0.1:24888".to_string(),
                    connected: true,
                },
            },
        );
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
                    "connected": true,
                    "session": {
                        "peerId": "linux-laptop",
                        "localDeviceId": "windows-desktop",
                        "address": "127.0.0.1:24888",
                        "connected": true
                    }
                }
            })
        );
    }

    #[test]
    fn session_disconnect_response_uses_camel_case_contract() {
        let response = JsonRpcSuccess::new(
            "req_1",
            SessionDisconnectResult {
                disconnected: true,
                session: Some(SessionStatus {
                    peer_id: "linux-laptop".to_string(),
                    local_device_id: "windows-desktop".to_string(),
                    address: "127.0.0.1:24888".to_string(),
                    connected: false,
                }),
                mode: ControlModeSnapshot::Local,
            },
        );
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
                    "disconnected": true,
                    "session": {
                        "peerId": "linux-laptop",
                        "localDeviceId": "windows-desktop",
                        "address": "127.0.0.1:24888",
                        "connected": false
                    },
                    "mode": "Local"
                }
            })
        );
    }

    #[test]
    fn in_process_client_sends_request_lines_to_server() {
        let endpoint = match IpcEndpoint::manual("in-process://test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let client = InProcessIpcClient::new(endpoint.clone(), EchoServer);

        assert_eq!(client.endpoint(), &endpoint);
        assert_eq!(
            client.send_request_line("request\n"),
            Ok("handled:request\n".to_string())
        );
    }

    #[test]
    fn local_json_rpc_call_encodes_request_before_transport_send() {
        let endpoint = match IpcEndpoint::manual("in-process://test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let client = InProcessIpcClient::new(endpoint, EchoServer);
        let request =
            JsonRpcRequest::new("req_1", METHOD_DAEMON_STATUS, DaemonStatusParams::default());
        let response_line = match call_json_rpc(&client, &request) {
            Ok(line) => line,
            Err(error) => panic!("expected local JSON-RPC response: {error}"),
        };

        assert_eq!(
            json_value_or_panic(&response_line[8..]),
            json!({
                "jsonrpc": "2.0",
                "id": "req_1",
                "method": "daemon.status",
                "params": {}
            })
        );
    }

    #[test]
    fn local_json_rpc_call_returns_transport_failures() {
        let endpoint = match IpcEndpoint::manual("in-process://test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let client = InProcessIpcClient::new(endpoint, FailingServer);
        let request =
            JsonRpcRequest::new("req_1", METHOD_DAEMON_STATUS, DaemonStatusParams::default());

        let error = match call_json_rpc(&client, &request) {
            Ok(response) => panic!("expected transport error, got {response}"),
            Err(error) => error,
        };

        assert_eq!(error.to_string(), "IPC request failed: server unavailable");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn os_local_ipc_client_and_server_exchange_one_request_line() {
        let endpoint = unique_os_endpoint();
        let server_endpoint = endpoint.clone();
        let server_thread = std::thread::spawn(move || {
            let server = EchoServer;
            serve_os_local_ipc_once(&server_endpoint, &server)
        });
        let client = OsLocalIpcClient::new(endpoint);

        let response = send_with_short_retry(&client, "ping\n");
        let server_result = match server_thread.join() {
            Ok(result) => result,
            Err(_) => panic!("expected OS IPC server thread to finish"),
        };

        assert_eq!(server_result, Ok(()));
        assert_eq!(response, Ok("handled:ping\n".to_string()));
    }

    #[test]
    fn parses_daemon_status_request_line() {
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
            parse_request_line(&line),
            Ok(IpcRequest::DaemonStatus(request))
        );
    }

    #[test]
    fn parses_diagnostics_screen_topology_request_line() {
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
            DiagnosticsScreenTopologyParams::default(),
        );
        let line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        assert_eq!(
            parse_request_line(&line),
            Ok(IpcRequest::DiagnosticsScreenTopology(request))
        );
    }

    #[test]
    fn parses_input_release_all_request_line() {
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_INPUT_RELEASE_ALL,
            super::InputReleaseAllParams::default(),
        );
        let line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        assert_eq!(
            parse_request_line(&line),
            Ok(IpcRequest::InputReleaseAll(request))
        );
    }

    #[test]
    fn parses_session_connect_request_line() {
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_SESSION_CONNECT,
            SessionConnectParams {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
        );
        let line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        assert_eq!(
            parse_request_line(&line),
            Ok(IpcRequest::SessionConnect(request))
        );
    }

    #[test]
    fn parses_session_disconnect_request_line() {
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_SESSION_DISCONNECT,
            SessionDisconnectParams::default(),
        );
        let line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        assert_eq!(
            parse_request_line(&line),
            Ok(IpcRequest::SessionDisconnect(request))
        );
    }

    #[test]
    fn malformed_request_line_returns_parse_error() {
        assert_eq!(
            parse_request_line("{not json"),
            Err(super::JsonRpcFailure::new(
                None,
                super::JsonRpcError::new(JSONRPC_ERROR_PARSE, "parse error")
            ))
        );
    }

    #[test]
    fn unknown_request_method_returns_method_not_found() {
        let line = r#"{"jsonrpc":"2.0","id":"req_1","method":"daemon.nope","params":{}}"#;

        assert_eq!(
            parse_request_line(line),
            Err(super::JsonRpcFailure::new(
                Some("req_1".to_string()),
                super::JsonRpcError::new(
                    JSONRPC_ERROR_METHOD_NOT_FOUND,
                    "method not found: daemon.nope"
                )
            ))
        );
    }

    #[test]
    fn invalid_request_params_return_invalid_params() {
        let line = r#"{"jsonrpc":"2.0","id":"req_1","method":"daemon.status","params":[]}"#;
        let failure = match parse_request_line(line) {
            Ok(request) => panic!("expected invalid params failure, got {request:?}"),
            Err(failure) => failure,
        };

        assert_eq!(failure.id, Some("req_1".to_string()));
        assert_eq!(failure.error.code, JSONRPC_ERROR_INVALID_PARAMS);
        assert!(
            failure
                .error
                .message
                .starts_with("invalid params for daemon.status:")
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
