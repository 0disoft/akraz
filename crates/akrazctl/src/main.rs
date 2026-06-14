use std::env;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::process::ExitCode;

use akraz_ipc::{
    DaemonStatusParams, InputReleaseAllParams, IpcCallError, IpcEndpoint, IpcEndpointError,
    IpcTransportError, JsonRpcRequest, METHOD_DAEMON_STATUS, METHOD_INPUT_RELEASE_ALL,
    METHOD_PERMISSIONS_PROBE, METHOD_SESSION_CONNECT, METHOD_SESSION_DISCONNECT, OsLocalIpcClient,
    PermissionsProbeParams, SessionConnectParams, SessionDisconnectParams, call_json_rpc,
    resolve_current_default_endpoint,
};

const LOCAL_REQUEST_ID: &str = "local";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        Some("--version") | Some("-V") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some("status") => match parse_endpoint_options(args) {
            Ok(options) => print_status(options),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(2)
            }
        },
        Some("daemon-args") => match parse_daemon_args_options(args) {
            Ok(options) => print_daemon_args(options),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(2)
            }
        },
        Some("permissions") => match args.next().as_deref() {
            Some("probe") => match parse_endpoint_options(args) {
                Ok(options) => print_permissions_probe(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown permissions command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing permissions command");
                ExitCode::from(2)
            }
        },
        Some("input") => match args.next().as_deref() {
            Some("release-all") => match parse_endpoint_options(args) {
                Ok(options) => print_input_release_all(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown input command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing input command");
                ExitCode::from(2)
            }
        },
        Some("session") => match args.next().as_deref() {
            Some("connect") => match parse_session_connect_options(args) {
                Ok(options) => print_session_connect(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some("disconnect") => match parse_endpoint_options(args) {
                Ok(options) => print_session_disconnect(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown session command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing session command");
                ExitCode::from(2)
            }
        },
        Some(argument) => {
            eprintln!("unknown command: {argument}");
            ExitCode::from(2)
        }
        None => {
            eprintln!(
                "usage: akrazctl <status|permissions probe|input release-all|session connect|session disconnect|daemon-args|--version>"
            );
            ExitCode::from(2)
        }
    }
}

fn print_version() {
    println!("akrazctl {}", env!("CARGO_PKG_VERSION"));
}

fn print_status(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_permissions_probe(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_PERMISSIONS_PROBE,
        PermissionsProbeParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_input_release_all(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_INPUT_RELEASE_ALL,
        InputReleaseAllParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_session_connect(options: SessionConnectOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_SESSION_CONNECT,
        SessionConnectParams {
            peer_id: options.peer_id,
            local_device_id: options.local_device_id,
            address: options.address,
        },
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_session_disconnect(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_SESSION_DISCONNECT,
        SessionDisconnectParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_daemon_args(options: DaemonArgsOptions) -> ExitCode {
    println!("{}", format_daemon_command_line(&options));
    ExitCode::SUCCESS
}

fn print_local_daemon_response<P>(
    endpoint: Option<IpcEndpoint>,
    request: &JsonRpcRequest<P>,
) -> ExitCode
where
    P: serde::Serialize,
{
    let client = match build_daemon_client(endpoint) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    match call_json_rpc(&client, request) {
        Ok(line) => {
            print!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", format_daemon_call_error(&error));
            ExitCode::FAILURE
        }
    }
}

fn format_daemon_call_error(error: &IpcCallError) -> String {
    match error {
        IpcCallError::Transport {
            source: IpcTransportError::EndpointUnavailable { endpoint, message },
        } => format!(
            "akraz daemon is not reachable at {endpoint}. Start akraz-daemon, or pass --endpoint to use a different IPC endpoint. Details: {message}"
        ),
        IpcCallError::Transport {
            source: IpcTransportError::RequestFailed { message },
        } => format!("akraz daemon IPC request failed. Details: {message}"),
        IpcCallError::Encode { source } => {
            format!("failed to encode daemon IPC request: {source}")
        }
    }
}

fn build_daemon_client(endpoint: Option<IpcEndpoint>) -> Result<OsLocalIpcClient, CliRuntimeError> {
    build_daemon_client_with_resolver(endpoint, resolve_current_default_endpoint)
}

fn build_daemon_client_with_resolver<F>(
    endpoint: Option<IpcEndpoint>,
    resolve_default_endpoint: F,
) -> Result<OsLocalIpcClient, CliRuntimeError>
where
    F: FnOnce() -> Result<IpcEndpoint, IpcEndpointError>,
{
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None => resolve_default_endpoint().map_err(CliRuntimeError::from)?,
    };

    Ok(OsLocalIpcClient::new(endpoint))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointOptions {
    endpoint: Option<IpcEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonArgsOptions {
    capture_input: bool,
    edge_bindings: Vec<String>,
    peer_listen: Option<String>,
    peer_session: Option<String>,
    local_device_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionConnectOptions {
    endpoint: Option<IpcEndpoint>,
    peer_id: String,
    local_device_id: String,
    address: String,
}

fn parse_endpoint_options<I>(args: I) -> Result<EndpointOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut endpoint = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--endpoint=") {
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else if argument == "--endpoint" {
            let value = args.next().ok_or(CliUsageError::MissingEndpointValue)?;
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else {
            return Err(CliUsageError::UnknownStatusOption(argument));
        }
    }

    Ok(EndpointOptions { endpoint })
}

fn parse_session_connect_options<I>(args: I) -> Result<SessionConnectOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut endpoint = None;
    let mut peer_id = None;
    let mut local_device_id = None;
    let mut address = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--endpoint=") {
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else if argument == "--endpoint" {
            let value = args.next().ok_or(CliUsageError::MissingEndpointValue)?;
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else if let Some(value) = argument.strip_prefix("--peer-id=") {
            set_once_daemon_option(
                "--peer-id",
                &mut peer_id,
                normalize_peer_id("--peer-id", value)?,
            )?;
        } else if argument == "--peer-id" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingSessionOptionValue("--peer-id"))?;
            let value = normalize_peer_id("--peer-id", &value)?;
            set_once_daemon_option("--peer-id", &mut peer_id, value)?;
        } else if let Some(value) = argument.strip_prefix("--local-device-id=") {
            set_once_daemon_option(
                "--local-device-id",
                &mut local_device_id,
                normalize_local_device_id_arg(value)?,
            )?;
        } else if argument == "--local-device-id" {
            let value = args.next().ok_or(CliUsageError::MissingSessionOptionValue(
                "--local-device-id",
            ))?;
            let value = normalize_local_device_id_arg(&value)?;
            set_once_daemon_option("--local-device-id", &mut local_device_id, value)?;
        } else if let Some(value) = argument.strip_prefix("--address=") {
            set_once_daemon_option(
                "--address",
                &mut address,
                normalize_socket_addr_arg("--address", value)?,
            )?;
        } else if argument == "--address" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingSessionOptionValue("--address"))?;
            let value = normalize_socket_addr_arg("--address", &value)?;
            set_once_daemon_option("--address", &mut address, value)?;
        } else {
            return Err(CliUsageError::UnknownSessionOption(argument));
        }
    }

    Ok(SessionConnectOptions {
        endpoint,
        peer_id: peer_id.ok_or(CliUsageError::MissingSessionConnectPeerId)?,
        local_device_id: local_device_id
            .ok_or(CliUsageError::MissingSessionConnectLocalDeviceId)?,
        address: address.ok_or(CliUsageError::MissingSessionConnectAddress)?,
    })
}

fn parse_daemon_args_options<I>(args: I) -> Result<DaemonArgsOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut options = DaemonArgsOptions {
        capture_input: false,
        edge_bindings: Vec::new(),
        peer_listen: None,
        peer_session: None,
        local_device_id: None,
    };
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if argument == "--capture-input" {
            options.capture_input = true;
        } else if let Some(value) = argument.strip_prefix("--edge-binding=") {
            options
                .edge_bindings
                .push(normalize_edge_binding_arg(value)?);
        } else if argument == "--edge-binding" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--edge-binding"))?;
            options
                .edge_bindings
                .push(normalize_edge_binding_arg(&value)?);
        } else if let Some(value) = argument.strip_prefix("--peer-listen=") {
            set_once_daemon_option(
                "--peer-listen",
                &mut options.peer_listen,
                normalize_socket_addr_arg("--peer-listen", value)?,
            )?;
        } else if argument == "--peer-listen" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--peer-listen"))?;
            let value = normalize_socket_addr_arg("--peer-listen", &value)?;
            set_once_daemon_option("--peer-listen", &mut options.peer_listen, value)?;
        } else if let Some(value) = argument.strip_prefix("--peer-session=") {
            set_once_daemon_option(
                "--peer-session",
                &mut options.peer_session,
                normalize_peer_session_arg(value)?,
            )?;
        } else if argument == "--peer-session" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--peer-session"))?;
            let value = normalize_peer_session_arg(&value)?;
            set_once_daemon_option("--peer-session", &mut options.peer_session, value)?;
        } else if let Some(value) = argument.strip_prefix("--local-device-id=") {
            set_once_daemon_option(
                "--local-device-id",
                &mut options.local_device_id,
                normalize_local_device_id_arg(value)?,
            )?;
        } else if argument == "--local-device-id" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--local-device-id"))?;
            let value = normalize_local_device_id_arg(&value)?;
            set_once_daemon_option("--local-device-id", &mut options.local_device_id, value)?;
        } else {
            return Err(CliUsageError::UnknownDaemonArgsOption(argument));
        }
    }

    if options.peer_session.is_some() && options.local_device_id.is_none() {
        return Err(CliUsageError::PeerSessionRequiresLocalDeviceId);
    }

    Ok(options)
}

fn set_once_daemon_option(
    option_name: &'static str,
    target: &mut Option<String>,
    value: String,
) -> Result<(), CliUsageError> {
    if target.is_some() {
        return Err(CliUsageError::DuplicateDaemonOption(option_name));
    }

    *target = Some(value);
    Ok(())
}

fn normalize_edge_binding_arg(value: &str) -> Result<String, CliUsageError> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: "--edge-binding",
            value: value.to_string(),
            reason: "expected <local-edge>:<peer-id>:<remote-edge>".to_string(),
        });
    }

    let local_edge = normalize_screen_edge("--edge-binding", parts[0])?;
    let peer_id = normalize_peer_id("--edge-binding", parts[1])?;
    let remote_edge = normalize_screen_edge("--edge-binding", parts[2])?;

    Ok(format!("{local_edge}:{peer_id}:{remote_edge}"))
}

fn normalize_screen_edge(option_name: &'static str, value: &str) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg(option_name, value)?;
    match value.to_ascii_lowercase().as_str() {
        "left" => Ok("left".to_string()),
        "right" => Ok("right".to_string()),
        "top" => Ok("top".to_string()),
        "bottom" => Ok("bottom".to_string()),
        _ => Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value,
            reason: "edge must be one of left, right, top, bottom".to_string(),
        }),
    }
}

fn normalize_peer_id(option_name: &'static str, value: &str) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg(option_name, value)?;
    if value.contains(':') || value.contains('@') {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value,
            reason: "peer id must not contain ':' or '@'".to_string(),
        });
    }

    Ok(value)
}

fn normalize_socket_addr_arg(
    option_name: &'static str,
    value: &str,
) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg(option_name, value)?;
    if value.parse::<SocketAddr>().is_err() {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value,
            reason: "expected <ip>:<port> socket address".to_string(),
        });
    }

    Ok(value)
}

fn normalize_peer_session_arg(value: &str) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg("--peer-session", value)?;
    let (peer_id, address) =
        value
            .split_once('@')
            .ok_or_else(|| CliUsageError::InvalidDaemonOptionValue {
                option: "--peer-session",
                value: value.clone(),
                reason: "expected <peer-id>@<ip>:<port>".to_string(),
            })?;
    if address.contains('@') {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: "--peer-session",
            value,
            reason: "expected exactly one '@' separator".to_string(),
        });
    }

    let peer_id = normalize_peer_id("--peer-session", peer_id)?;
    let address = normalize_socket_addr_arg("--peer-session", address)?;

    Ok(format!("{peer_id}@{address}"))
}

fn normalize_local_device_id_arg(value: &str) -> Result<String, CliUsageError> {
    normalize_shell_safe_arg("--local-device-id", value)
}

fn normalize_shell_safe_arg(
    option_name: &'static str,
    value: &str,
) -> Result<String, CliUsageError> {
    if value.is_empty() {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "value must not be empty".to_string(),
        });
    }
    if value.trim() != value {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "value must not start or end with whitespace".to_string(),
        });
    }
    if value.chars().any(char::is_whitespace) {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "value must not contain whitespace".to_string(),
        });
    }

    Ok(value.to_string())
}

fn format_daemon_command_line(options: &DaemonArgsOptions) -> String {
    let mut command = vec!["akraz-daemon".to_string(), "--serve".to_string()];
    if options.capture_input {
        command.push("--capture-input".to_string());
    }
    for edge_binding in &options.edge_bindings {
        command.push("--edge-binding".to_string());
        command.push(edge_binding.clone());
    }
    if let Some(peer_listen) = &options.peer_listen {
        command.push("--peer-listen".to_string());
        command.push(peer_listen.clone());
    }
    if let Some(local_device_id) = &options.local_device_id {
        command.push("--local-device-id".to_string());
        command.push(local_device_id.clone());
    }
    if let Some(peer_session) = &options.peer_session {
        command.push("--peer-session".to_string());
        command.push(peer_session.clone());
    }

    command.join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliUsageError {
    MissingEndpointValue,
    MissingDaemonOptionValue(&'static str),
    MissingSessionOptionValue(&'static str),
    MissingSessionConnectPeerId,
    MissingSessionConnectLocalDeviceId,
    MissingSessionConnectAddress,
    DuplicateDaemonOption(&'static str),
    PeerSessionRequiresLocalDeviceId,
    InvalidEndpoint(String),
    InvalidDaemonOptionValue {
        option: &'static str,
        value: String,
        reason: String,
    },
    UnknownStatusOption(String),
    UnknownSessionOption(String),
    UnknownDaemonArgsOption(String),
}

impl From<akraz_ipc::IpcEndpointError> for CliUsageError {
    fn from(error: akraz_ipc::IpcEndpointError) -> Self {
        Self::InvalidEndpoint(error.to_string())
    }
}

impl Display for CliUsageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEndpointValue => formatter.write_str("missing value for --endpoint"),
            Self::MissingDaemonOptionValue(option) => {
                write!(formatter, "missing value for {option}")
            }
            Self::MissingSessionOptionValue(option) => {
                write!(formatter, "missing value for {option}")
            }
            Self::MissingSessionConnectPeerId => {
                formatter.write_str("session connect requires --peer-id")
            }
            Self::MissingSessionConnectLocalDeviceId => {
                formatter.write_str("session connect requires --local-device-id")
            }
            Self::MissingSessionConnectAddress => {
                formatter.write_str("session connect requires --address")
            }
            Self::DuplicateDaemonOption(option) => {
                write!(formatter, "{option} can only be provided once")
            }
            Self::PeerSessionRequiresLocalDeviceId => {
                formatter.write_str("--peer-session requires --local-device-id")
            }
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
            Self::InvalidDaemonOptionValue {
                option,
                value,
                reason,
            } => {
                write!(formatter, "invalid value for {option}: {value} ({reason})")
            }
            Self::UnknownStatusOption(argument) => {
                write!(formatter, "unknown status option: {argument}")
            }
            Self::UnknownSessionOption(argument) => {
                write!(formatter, "unknown session option: {argument}")
            }
            Self::UnknownDaemonArgsOption(argument) => {
                write!(formatter, "unknown daemon-args option: {argument}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliRuntimeError {
    InvalidEndpoint(String),
}

impl From<akraz_ipc::IpcEndpointError> for CliRuntimeError {
    fn from(error: akraz_ipc::IpcEndpointError) -> Self {
        Self::InvalidEndpoint(error.to_string())
    }
}

impl Display for CliRuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use akraz_ipc::{
        IpcCallError, IpcEndpoint, IpcEndpointError, IpcEndpointKind, IpcTransportError,
        JsonRpcRequest, LocalIpcClient,
    };

    use super::{
        CliRuntimeError, CliUsageError, DaemonArgsOptions, EndpointOptions, LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS, METHOD_SESSION_CONNECT, METHOD_SESSION_DISCONNECT,
        SessionConnectOptions, build_daemon_client_with_resolver, format_daemon_call_error,
        format_daemon_command_line, parse_daemon_args_options, parse_endpoint_options,
        parse_session_connect_options,
    };
    use akraz_ipc::METHOD_INPUT_RELEASE_ALL;

    #[test]
    fn parses_endpoint_option() {
        assert_eq!(
            parse_endpoint_options(["--endpoint", "local-test"].map(String::from)),
            Ok(EndpointOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
            })
        );
        assert_eq!(
            parse_endpoint_options(["--endpoint=local-test"].map(String::from)),
            Ok(EndpointOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
            })
        );
    }

    #[test]
    fn rejects_invalid_endpoint_options() {
        assert_eq!(
            parse_endpoint_options(["--endpoint"].map(String::from)),
            Err(CliUsageError::MissingEndpointValue)
        );
        assert_eq!(
            parse_endpoint_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownStatusOption("--bad".to_string()))
        );
    }

    #[test]
    fn parses_session_connect_options() {
        assert_eq!(
            parse_session_connect_options(
                [
                    "--endpoint",
                    "local-test",
                    "--peer-id",
                    "linux-laptop",
                    "--local-device-id",
                    "windows-desktop",
                    "--address",
                    "127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(SessionConnectOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            })
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--peer-id=linux-laptop",
                    "--local-device-id=windows-desktop",
                    "--address=127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(SessionConnectOptions {
                endpoint: None,
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            })
        );
    }

    #[test]
    fn rejects_invalid_session_connect_options() {
        assert_eq!(
            parse_session_connect_options(["--peer-id"].map(String::from)),
            Err(CliUsageError::MissingSessionOptionValue("--peer-id"))
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--local-device-id",
                    "windows-desktop",
                    "--address",
                    "127.0.0.1:24888"
                ]
                .map(String::from)
            ),
            Err(CliUsageError::MissingSessionConnectPeerId)
        );
        assert_eq!(
            parse_session_connect_options(
                ["--peer-id", "linux-laptop", "--address", "127.0.0.1:24888"].map(String::from)
            ),
            Err(CliUsageError::MissingSessionConnectLocalDeviceId)
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--peer-id",
                    "linux-laptop",
                    "--local-device-id",
                    "windows-desktop"
                ]
                .map(String::from)
            ),
            Err(CliUsageError::MissingSessionConnectAddress)
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--peer-id",
                    "linux-laptop",
                    "--local-device-id",
                    "windows-desktop",
                    "--address",
                    "bad-address",
                ]
                .map(String::from)
            ),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--address",
                value: "bad-address".to_string(),
                reason: "expected <ip>:<port> socket address".to_string(),
            })
        );
        assert_eq!(
            parse_session_connect_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownSessionOption("--bad".to_string()))
        );
    }

    #[test]
    fn parses_manual_source_daemon_args() {
        let options = parse_daemon_args_options(
            [
                "--capture-input",
                "--edge-binding",
                "RIGHT:linux-laptop:LEFT",
                "--local-device-id",
                "windows-desktop",
                "--peer-session",
                "linux-laptop@127.0.0.1:24888",
            ]
            .map(String::from),
        );

        assert_eq!(
            options,
            Ok(DaemonArgsOptions {
                capture_input: true,
                edge_bindings: vec!["right:linux-laptop:left".to_string()],
                peer_listen: None,
                peer_session: Some("linux-laptop@127.0.0.1:24888".to_string()),
                local_device_id: Some("windows-desktop".to_string()),
            })
        );
    }

    #[test]
    fn parses_manual_target_daemon_args() {
        let options = parse_daemon_args_options(["--peer-listen=0.0.0.0:24888"].map(String::from));

        assert_eq!(
            options,
            Ok(DaemonArgsOptions {
                capture_input: false,
                edge_bindings: Vec::new(),
                peer_listen: Some("0.0.0.0:24888".to_string()),
                peer_session: None,
                local_device_id: None,
            })
        );
    }

    #[test]
    fn formats_manual_daemon_command_line() {
        let options = DaemonArgsOptions {
            capture_input: true,
            edge_bindings: vec!["right:linux-laptop:left".to_string()],
            peer_listen: Some("127.0.0.1:24887".to_string()),
            peer_session: Some("linux-laptop@127.0.0.1:24888".to_string()),
            local_device_id: Some("windows-desktop".to_string()),
        };

        assert_eq!(
            format_daemon_command_line(&options),
            "akraz-daemon --serve --capture-input --edge-binding right:linux-laptop:left --peer-listen 127.0.0.1:24887 --local-device-id windows-desktop --peer-session linux-laptop@127.0.0.1:24888"
        );
    }

    #[test]
    fn rejects_invalid_manual_daemon_args() {
        assert_eq!(
            parse_daemon_args_options(["--edge-binding"].map(String::from)),
            Err(CliUsageError::MissingDaemonOptionValue("--edge-binding"))
        );
        assert_eq!(
            parse_daemon_args_options(["--peer-listen", "not-an-address"].map(String::from)),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--peer-listen",
                value: "not-an-address".to_string(),
                reason: "expected <ip>:<port> socket address".to_string(),
            })
        );
        assert_eq!(
            parse_daemon_args_options(
                ["--peer-session", "linux-laptop@127.0.0.1:24888"].map(String::from)
            ),
            Err(CliUsageError::PeerSessionRequiresLocalDeviceId)
        );
        assert_eq!(
            parse_daemon_args_options(
                [
                    "--local-device-id",
                    "windows-desktop",
                    "--peer-session",
                    "linux-laptop@bad-address",
                ]
                .map(String::from)
            ),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--peer-session",
                value: "bad-address".to_string(),
                reason: "expected <ip>:<port> socket address".to_string(),
            })
        );
        assert_eq!(
            parse_daemon_args_options(["--edge-binding", "east:peer:left"].map(String::from)),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--edge-binding",
                value: "east".to_string(),
                reason: "edge must be one of left, right, top, bottom".to_string(),
            })
        );
        assert_eq!(
            parse_daemon_args_options(
                [
                    "--peer-listen",
                    "127.0.0.1:24887",
                    "--peer-listen=127.0.0.1:24888"
                ]
                .map(String::from)
            ),
            Err(CliUsageError::DuplicateDaemonOption("--peer-listen"))
        );
        assert_eq!(
            parse_daemon_args_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownDaemonArgsOption("--bad".to_string()))
        );
    }

    #[test]
    fn status_request_uses_daemon_status_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DAEMON_STATUS,
            akraz_ipc::DaemonStatusParams::default(),
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_DAEMON_STATUS);
    }

    #[test]
    fn release_all_request_uses_input_release_all_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_INPUT_RELEASE_ALL,
            akraz_ipc::InputReleaseAllParams::default(),
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_INPUT_RELEASE_ALL);
    }

    #[test]
    fn session_connect_request_uses_session_connect_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_SESSION_CONNECT,
            akraz_ipc::SessionConnectParams {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_SESSION_CONNECT);
    }

    #[test]
    fn session_disconnect_request_uses_session_disconnect_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_SESSION_DISCONNECT,
            akraz_ipc::SessionDisconnectParams::default(),
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_SESSION_DISCONNECT);
    }

    #[test]
    fn default_daemon_client_resolves_os_endpoint() {
        let endpoint = match IpcEndpoint::manual("resolved-endpoint") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let expected_endpoint = endpoint.clone();
        let client = match build_daemon_client_with_resolver(None, || Ok(endpoint)) {
            Ok(client) => client,
            Err(error) => panic!("expected daemon IPC client: {error}"),
        };

        assert_eq!(client.endpoint(), &expected_endpoint);
    }

    #[test]
    fn default_daemon_client_reports_endpoint_resolution_errors() {
        let error = build_daemon_client_with_resolver(None, || {
            Err(IpcEndpointError::UnsupportedOperatingSystem)
        });

        match error {
            Err(CliRuntimeError::InvalidEndpoint(message)) => {
                assert_eq!(message, "unsupported operating system for local IPC")
            }
            Ok(client) => panic!("expected endpoint resolution failure, got {client:?}"),
        }
    }

    #[test]
    fn explicit_endpoint_selects_os_ipc_client_without_resolving_default() {
        let endpoint = match IpcEndpoint::manual("explicit-endpoint") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let client = match build_daemon_client_with_resolver(Some(endpoint.clone()), || {
            Err(IpcEndpointError::UnsupportedOperatingSystem)
        }) {
            Ok(client) => client,
            Err(error) => panic!("expected daemon IPC client: {error}"),
        };

        assert_eq!(client.endpoint(), &endpoint);
    }

    #[test]
    fn daemon_call_error_reports_unreachable_endpoint_with_next_action() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = IpcCallError::Transport {
            source: IpcTransportError::endpoint_unavailable(
                endpoint,
                "No process is on the other end of the pipe.",
            ),
        };

        assert_eq!(
            format_daemon_call_error(&error),
            "akraz daemon is not reachable at local-test. Start akraz-daemon, or pass --endpoint to use a different IPC endpoint. Details: No process is on the other end of the pipe."
        );
    }

    #[test]
    fn daemon_call_error_reports_request_failures_as_ipc_failures() {
        let error = IpcCallError::Transport {
            source: IpcTransportError::request_failed("pipe closed before a response line"),
        };

        assert_eq!(
            format_daemon_call_error(&error),
            "akraz daemon IPC request failed. Details: pipe closed before a response line"
        );
    }
}
