use std::env;
use std::fmt::{Display, Formatter};
use std::net::TcpListener;
use std::process::ExitCode;
use std::thread;

use akraz_core::{
    CoreAction, EdgeCrossing, InjectedInputEvent, LogicalPoint, MouseButton, PeerId, PhysicalKey,
    PressState, RuntimeInputState, ScreenEdge, ScreenEdgeBinding, SessionId,
};
use akraz_daemon::{
    CoreActionDispatcher, DaemonInputCaptureConfig, DaemonInputCaptureWorker, DaemonIpcRunConfig,
    DaemonIpcServer, DaemonTransportCommand, LoopbackPeerTransport, TcpPeerTransport,
    TransportCoreActionDispatcher, build_daemon_status, serve_daemon_ipc,
    serve_tcp_peer_transport_commands, start_daemon_input_capture_with_edge_bindings,
};
use akraz_ipc::{IpcEndpoint, IpcTransportError, resolve_current_default_endpoint};
use akraz_platform::runtime_platform_adapter;
use serde::Serialize;

fn main() -> ExitCode {
    match parse_daemon_command(env::args().skip(1)) {
        Ok(DaemonCommand::Version) => {
            print_version();
            ExitCode::SUCCESS
        }
        Ok(DaemonCommand::Status) => {
            print_status();
            ExitCode::SUCCESS
        }
        Ok(DaemonCommand::LoopbackTransportSmoke) => run_loopback_transport_smoke(),
        Ok(DaemonCommand::TcpTransportSmoke) => run_tcp_transport_smoke(),
        Ok(DaemonCommand::Serve(options)) => run_daemon(options),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn print_version() {
    println!("akraz-daemon {}", env!("CARGO_PKG_VERSION"));
}

fn run_daemon(options: ServeOptions) -> ExitCode {
    let endpoint = match options.endpoint {
        Some(endpoint) => endpoint,
        None => match resolve_current_default_endpoint() {
            Ok(endpoint) => endpoint,
            Err(error) => {
                eprintln!("failed to resolve daemon IPC endpoint: {error}");
                return ExitCode::FAILURE;
            }
        },
    };
    let config = if options.once {
        DaemonIpcRunConfig::serve_requests(endpoint, 1)
    } else {
        DaemonIpcRunConfig::serve_forever(endpoint)
    };
    let platform = runtime_platform_adapter();
    let server = DaemonIpcServer::new(RuntimeInputState::new(), platform);
    let capture_worker = if options.capture_input {
        match start_daemon_input_capture_with_edge_bindings(
            server.shared_state(),
            &platform,
            DaemonInputCaptureConfig::default(),
            options.edge_bindings.clone(),
        ) {
            Ok(worker) => Some(worker),
            Err(error) => {
                eprintln!("failed to start daemon input capture: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    eprintln!("akraz-daemon listening at {}", config.endpoint());
    let result = match serve_daemon_ipc(&config, &server) {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("{}", format_daemon_ipc_error(&error));
            Err(())
        }
    };

    match stop_capture_worker(capture_worker) {
        Ok(()) if result.is_ok() => ExitCode::SUCCESS,
        Ok(()) | Err(()) => ExitCode::FAILURE,
    }
}

fn stop_capture_worker(worker: Option<DaemonInputCaptureWorker>) -> Result<(), ()> {
    let Some(worker) = worker else {
        return Ok(());
    };

    match worker.stop() {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("failed to stop daemon input capture: {error}");
            Err(())
        }
    }
}

fn format_daemon_ipc_error(error: &IpcTransportError) -> String {
    match error {
        IpcTransportError::EndpointUnavailable { endpoint, message } => format!(
            "failed to open daemon IPC endpoint at {endpoint}. Another akraz-daemon may already be running, or the endpoint path may be unavailable. Details: {message}"
        ),
        IpcTransportError::RequestFailed { message } => {
            format!("daemon IPC request failed. Details: {message}")
        }
    }
}

fn print_status() {
    let state = RuntimeInputState::new();
    let platform = runtime_platform_adapter();
    let status = match build_daemon_status(&state, &platform) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("failed to build daemon status: {error}");
            return;
        }
    };

    println!("akraz-daemon {}", status.daemon_version);
    println!("mode: {:?}", status.mode);
    println!(
        "protocol: {}.{}",
        status.protocol.major, status.protocol.minor
    );
}

fn run_loopback_transport_smoke() -> ExitCode {
    match build_loopback_transport_smoke_report() {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("failed to encode loopback transport smoke report: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("loopback transport smoke failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_tcp_transport_smoke() -> ExitCode {
    match build_tcp_transport_smoke_report() {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("failed to encode TCP transport smoke report: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("TCP transport smoke failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_loopback_transport_smoke_report()
-> Result<LoopbackTransportSmokeReport, akraz_platform::PlatformError> {
    let transport = LoopbackPeerTransport::new();
    let dispatcher = TransportCoreActionDispatcher::new(transport.clone());

    dispatcher.dispatch_core_actions(&loopback_transport_smoke_actions())?;

    Ok(LoopbackTransportSmokeReport {
        daemon_version: env!("CARGO_PKG_VERSION"),
        commands: transport
            .snapshot()?
            .iter()
            .map(LoopbackTransportSmokeCommand::from)
            .collect(),
    })
}

fn build_tcp_transport_smoke_report()
-> Result<LoopbackTransportSmokeReport, akraz_platform::PlatformError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
        akraz_platform::PlatformError::new(format!(
            "failed to bind TCP transport smoke listener: {error}"
        ))
    })?;
    let address = listener.local_addr().map_err(|error| {
        akraz_platform::PlatformError::new(format!(
            "failed to read TCP transport smoke listener address: {error}"
        ))
    })?;
    let server_thread = thread::spawn(move || {
        serve_tcp_peer_transport_commands(&listener, loopback_transport_smoke_actions().len())
    });
    let transport = TcpPeerTransport::new(PeerId::new("loopback-peer"), address);
    let dispatcher = TransportCoreActionDispatcher::new(transport);

    dispatcher.dispatch_core_actions(&loopback_transport_smoke_actions())?;

    let commands = server_thread
        .join()
        .map_err(|_| akraz_platform::PlatformError::new("TCP transport smoke server panicked"))??;

    Ok(LoopbackTransportSmokeReport {
        daemon_version: env!("CARGO_PKG_VERSION"),
        commands: commands
            .iter()
            .map(LoopbackTransportSmokeCommand::from)
            .collect(),
    })
}

fn loopback_transport_smoke_actions() -> [CoreAction; 4] {
    let crossing = EdgeCrossing {
        peer_id: PeerId::new("loopback-peer"),
        local_edge: ScreenEdge::Right,
        remote_edge: ScreenEdge::Left,
        exit_position: LogicalPoint { x: 1920, y: 540 },
        edge_offset: 540,
    };

    [
        CoreAction::StartRemoteSession {
            peer_id: PeerId::new("loopback-peer"),
            crossing: Some(crossing),
        },
        CoreAction::ForwardInput {
            event: InjectedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            },
        },
        CoreAction::ReleaseAllInputs,
        CoreAction::StopRemoteSession {
            session_id: Some(SessionId::new("loopback-session")),
        },
    ]
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LoopbackTransportSmokeReport {
    daemon_version: &'static str,
    commands: Vec<LoopbackTransportSmokeCommand>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum LoopbackTransportSmokeCommand {
    StartRemoteSession {
        peer_id: String,
        crossing: Option<LoopbackTransportSmokeCrossing>,
    },
    ForwardInput {
        event: LoopbackTransportSmokeInputEvent,
    },
    ReleaseAllInputs,
    StopRemoteSession {
        session_id: Option<String>,
    },
}

impl From<&DaemonTransportCommand> for LoopbackTransportSmokeCommand {
    fn from(command: &DaemonTransportCommand) -> Self {
        match command {
            DaemonTransportCommand::StartRemoteSession { peer_id, crossing } => {
                Self::StartRemoteSession {
                    peer_id: peer_id.as_str().to_string(),
                    crossing: crossing.as_ref().map(LoopbackTransportSmokeCrossing::from),
                }
            }
            DaemonTransportCommand::ForwardInput { event } => Self::ForwardInput {
                event: LoopbackTransportSmokeInputEvent::from(event),
            },
            DaemonTransportCommand::ReleaseAllInputs => Self::ReleaseAllInputs,
            DaemonTransportCommand::StopRemoteSession { session_id } => Self::StopRemoteSession {
                session_id: session_id
                    .as_ref()
                    .map(|session_id| session_id.as_str().to_string()),
            },
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LoopbackTransportSmokeCrossing {
    peer_id: String,
    local_edge: &'static str,
    remote_edge: &'static str,
    exit_x: i32,
    exit_y: i32,
    edge_offset: i32,
}

impl From<&EdgeCrossing> for LoopbackTransportSmokeCrossing {
    fn from(crossing: &EdgeCrossing) -> Self {
        Self {
            peer_id: crossing.peer_id.as_str().to_string(),
            local_edge: screen_edge_name(crossing.local_edge),
            remote_edge: screen_edge_name(crossing.remote_edge),
            exit_x: crossing.exit_position.x,
            exit_y: crossing.exit_position.y,
            edge_offset: crossing.edge_offset,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum LoopbackTransportSmokeInputEvent {
    Key { key: String, state: &'static str },
    MouseButton { button: String, state: &'static str },
    PointerMoved { delta_x: i32, delta_y: i32 },
}

impl From<&InjectedInputEvent> for LoopbackTransportSmokeInputEvent {
    fn from(event: &InjectedInputEvent) -> Self {
        match event {
            InjectedInputEvent::Key { key, state } => Self::Key {
                key: physical_key_name(*key),
                state: press_state_name(*state),
            },
            InjectedInputEvent::MouseButton { button, state } => Self::MouseButton {
                button: mouse_button_name(*button),
                state: press_state_name(*state),
            },
            InjectedInputEvent::PointerMoved { delta_x, delta_y } => Self::PointerMoved {
                delta_x: *delta_x,
                delta_y: *delta_y,
            },
        }
    }
}

fn screen_edge_name(edge: ScreenEdge) -> &'static str {
    match edge {
        ScreenEdge::Left => "left",
        ScreenEdge::Right => "right",
        ScreenEdge::Top => "top",
        ScreenEdge::Bottom => "bottom",
    }
}

fn press_state_name(state: PressState) -> &'static str {
    match state {
        PressState::Pressed => "pressed",
        PressState::Released => "released",
    }
}

fn physical_key_name(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::LeftShift => "leftShift".to_string(),
        PhysicalKey::RightShift => "rightShift".to_string(),
        PhysicalKey::LeftControl => "leftControl".to_string(),
        PhysicalKey::RightControl => "rightControl".to_string(),
        PhysicalKey::LeftAlt => "leftAlt".to_string(),
        PhysicalKey::RightAlt => "rightAlt".to_string(),
        PhysicalKey::LeftMeta => "leftMeta".to_string(),
        PhysicalKey::RightMeta => "rightMeta".to_string(),
        PhysicalKey::Code(code) => format!("code:{code}"),
    }
}

fn mouse_button_name(button: MouseButton) -> String {
    match button {
        MouseButton::Left => "left".to_string(),
        MouseButton::Right => "right".to_string(),
        MouseButton::Middle => "middle".to_string(),
        MouseButton::Back => "back".to_string(),
        MouseButton::Forward => "forward".to_string(),
        MouseButton::Other(code) => format!("other:{code}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonCommand {
    Serve(ServeOptions),
    Status,
    Version,
    LoopbackTransportSmoke,
    TcpTransportSmoke,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ServeOptions {
    endpoint: Option<IpcEndpoint>,
    once: bool,
    capture_input: bool,
    edge_bindings: Vec<ScreenEdgeBinding>,
}

fn parse_daemon_command<I>(args: I) -> Result<DaemonCommand, DaemonUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(DaemonCommand::Serve(ServeOptions::default()));
    };

    match first.as_str() {
        "--version" | "-V" => reject_trailing_args(args, DaemonCommand::Version),
        "--status" => reject_trailing_args(args, DaemonCommand::Status),
        "--akraz-smoke-loopback-transport" => {
            reject_trailing_args(args, DaemonCommand::LoopbackTransportSmoke)
        }
        "--akraz-smoke-tcp-transport" => {
            reject_trailing_args(args, DaemonCommand::TcpTransportSmoke)
        }
        "--serve" => parse_serve_options(args),
        argument
            if argument.starts_with("--endpoint")
                || argument == "--once"
                || argument == "--capture-input"
                || argument.starts_with("--edge-binding") =>
        {
            parse_serve_options(std::iter::once(first).chain(args))
        }
        argument => Err(DaemonUsageError::UnknownArgument(argument.to_string())),
    }
}

fn reject_trailing_args<I>(
    args: I,
    command: DaemonCommand,
) -> Result<DaemonCommand, DaemonUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    if let Some(argument) = args.next() {
        Err(DaemonUsageError::UnknownArgument(argument))
    } else {
        Ok(command)
    }
}

fn parse_serve_options<I>(args: I) -> Result<DaemonCommand, DaemonUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut options = ServeOptions::default();
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if argument == "--once" {
            options.once = true;
        } else if argument == "--capture-input" {
            options.capture_input = true;
        } else if let Some(value) = argument.strip_prefix("--edge-binding=") {
            options.edge_bindings.push(parse_edge_binding(value)?);
        } else if argument == "--edge-binding" {
            let value = args
                .next()
                .ok_or(DaemonUsageError::MissingEdgeBindingValue)?;
            options.edge_bindings.push(parse_edge_binding(&value)?);
        } else if let Some(value) = argument.strip_prefix("--endpoint=") {
            options.endpoint = Some(IpcEndpoint::manual(value).map_err(DaemonUsageError::from)?);
        } else if argument == "--endpoint" {
            let value = args.next().ok_or(DaemonUsageError::MissingEndpointValue)?;
            options.endpoint = Some(IpcEndpoint::manual(value).map_err(DaemonUsageError::from)?);
        } else {
            return Err(DaemonUsageError::UnknownArgument(argument));
        }
    }

    Ok(DaemonCommand::Serve(options))
}

fn parse_edge_binding(value: &str) -> Result<ScreenEdgeBinding, DaemonUsageError> {
    let mut parts = value.split(':');
    let Some(local_edge) = parts.next() else {
        return Err(DaemonUsageError::InvalidEdgeBinding(value.to_string()));
    };
    let Some(peer_id) = parts.next() else {
        return Err(DaemonUsageError::InvalidEdgeBinding(value.to_string()));
    };
    let Some(remote_edge) = parts.next() else {
        return Err(DaemonUsageError::InvalidEdgeBinding(value.to_string()));
    };
    if parts.next().is_some() || peer_id.trim().is_empty() {
        return Err(DaemonUsageError::InvalidEdgeBinding(value.to_string()));
    }

    Ok(ScreenEdgeBinding {
        local_edge: parse_screen_edge(local_edge)?,
        peer_id: PeerId::new(peer_id.trim()),
        remote_edge: parse_screen_edge(remote_edge)?,
    })
}

fn parse_screen_edge(value: &str) -> Result<ScreenEdge, DaemonUsageError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Ok(ScreenEdge::Left),
        "right" => Ok(ScreenEdge::Right),
        "top" => Ok(ScreenEdge::Top),
        "bottom" => Ok(ScreenEdge::Bottom),
        _ => Err(DaemonUsageError::InvalidEdgeBinding(value.to_string())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonUsageError {
    MissingEndpointValue,
    MissingEdgeBindingValue,
    InvalidEndpoint(String),
    InvalidEdgeBinding(String),
    UnknownArgument(String),
}

impl From<akraz_ipc::IpcEndpointError> for DaemonUsageError {
    fn from(error: akraz_ipc::IpcEndpointError) -> Self {
        Self::InvalidEndpoint(error.to_string())
    }
}

impl Display for DaemonUsageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEndpointValue => formatter.write_str("missing value for --endpoint"),
            Self::MissingEdgeBindingValue => {
                formatter.write_str("missing value for --edge-binding")
            }
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
            Self::InvalidEdgeBinding(value) => write!(
                formatter,
                "invalid edge binding: {value}. Expected <local-edge>:<peer-id>:<remote-edge>"
            ),
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument: {argument}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use akraz_core::{PeerId, ScreenEdge, ScreenEdgeBinding};
    use akraz_ipc::{IpcEndpoint, IpcEndpointKind, IpcTransportError};

    use super::{
        DaemonCommand, DaemonUsageError, LoopbackTransportSmokeCommand,
        LoopbackTransportSmokeInputEvent, ServeOptions, build_loopback_transport_smoke_report,
        build_tcp_transport_smoke_report, format_daemon_ipc_error, parse_daemon_command,
    };

    #[test]
    fn default_command_serves_forever_on_default_endpoint() {
        assert_eq!(
            parse_daemon_command(std::iter::empty::<String>()),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
                capture_input: false,
                edge_bindings: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_serve_endpoint_once_and_capture_options() {
        assert_eq!(
            parse_daemon_command(
                [
                    "--serve",
                    "--endpoint",
                    "local-test",
                    "--once",
                    "--capture-input"
                ]
                .map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
                once: true,
                capture_input: true,
                edge_bindings: Vec::new(),
            }))
        );
        assert_eq!(
            parse_daemon_command(
                ["--endpoint=local-test", "--once", "--capture-input"].map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
                once: true,
                capture_input: true,
                edge_bindings: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_edge_binding_options() {
        let binding = ScreenEdgeBinding {
            local_edge: ScreenEdge::Right,
            peer_id: PeerId::new("linux-laptop"),
            remote_edge: ScreenEdge::Left,
        };

        assert_eq!(
            parse_daemon_command(
                [
                    "--serve",
                    "--capture-input",
                    "--edge-binding",
                    "right:linux-laptop:left"
                ]
                .map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
                capture_input: true,
                edge_bindings: vec![binding.clone()],
            }))
        );
        assert_eq!(
            parse_daemon_command(
                ["--capture-input", "--edge-binding=RIGHT:linux-laptop:LEFT"].map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
                capture_input: true,
                edge_bindings: vec![binding],
            }))
        );
    }

    #[test]
    fn parses_status_and_version_commands() {
        assert_eq!(
            parse_daemon_command(["--status"].map(String::from)),
            Ok(DaemonCommand::Status)
        );
        assert_eq!(
            parse_daemon_command(["--version"].map(String::from)),
            Ok(DaemonCommand::Version)
        );
    }

    #[test]
    fn parses_hidden_loopback_transport_smoke_command() {
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-loopback-transport"].map(String::from)),
            Ok(DaemonCommand::LoopbackTransportSmoke)
        );
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-loopback-transport", "--once"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--once".to_string()))
        );
    }

    #[test]
    fn parses_hidden_tcp_transport_smoke_command() {
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-tcp-transport"].map(String::from)),
            Ok(DaemonCommand::TcpTransportSmoke)
        );
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-tcp-transport", "--once"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--once".to_string()))
        );
    }

    #[test]
    fn loopback_transport_smoke_report_covers_transport_commands() {
        let report =
            build_loopback_transport_smoke_report().expect("loopback transport smoke report");

        assert_eq!(report.daemon_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.commands.len(), 4);
        assert!(matches!(
            &report.commands[0],
            LoopbackTransportSmokeCommand::StartRemoteSession {
                peer_id,
                crossing: Some(_),
            } if peer_id == "loopback-peer"
        ));
        assert_eq!(
            report.commands[1],
            LoopbackTransportSmokeCommand::ForwardInput {
                event: LoopbackTransportSmokeInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            }
        );
        assert_eq!(
            report.commands[2],
            LoopbackTransportSmokeCommand::ReleaseAllInputs
        );
        assert!(matches!(
            &report.commands[3],
            LoopbackTransportSmokeCommand::StopRemoteSession {
                session_id: Some(session_id),
            } if session_id == "loopback-session"
        ));
    }

    #[test]
    fn tcp_transport_smoke_report_covers_network_transport_commands() {
        let report = build_tcp_transport_smoke_report().expect("TCP transport smoke report");

        assert_eq!(report.daemon_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.commands.len(), 4);
        assert!(matches!(
            &report.commands[0],
            LoopbackTransportSmokeCommand::StartRemoteSession {
                peer_id,
                crossing: Some(_),
            } if peer_id == "loopback-peer"
        ));
        assert_eq!(
            report.commands[1],
            LoopbackTransportSmokeCommand::ForwardInput {
                event: LoopbackTransportSmokeInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            }
        );
        assert_eq!(
            report.commands[2],
            LoopbackTransportSmokeCommand::ReleaseAllInputs
        );
        assert!(matches!(
            &report.commands[3],
            LoopbackTransportSmokeCommand::StopRemoteSession {
                session_id: Some(session_id),
            } if session_id == "loopback-session"
        ));
    }

    #[test]
    fn rejects_invalid_daemon_options() {
        assert_eq!(
            parse_daemon_command(["--endpoint"].map(String::from)),
            Err(DaemonUsageError::MissingEndpointValue)
        );
        assert_eq!(
            parse_daemon_command(["--edge-binding"].map(String::from)),
            Err(DaemonUsageError::MissingEdgeBindingValue)
        );
        assert_eq!(
            parse_daemon_command(["--edge-binding", "right::left"].map(String::from)),
            Err(DaemonUsageError::InvalidEdgeBinding(
                "right::left".to_string()
            ))
        );
        assert_eq!(
            parse_daemon_command(["--edge-binding", "east:peer:left"].map(String::from)),
            Err(DaemonUsageError::InvalidEdgeBinding("east".to_string()))
        );
        assert_eq!(
            parse_daemon_command(["--status", "--once"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--once".to_string()))
        );
        assert_eq!(
            parse_daemon_command(["--bad"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--bad".to_string()))
        );
    }

    #[test]
    fn daemon_ipc_error_reports_unavailable_endpoint_with_lifecycle_hint() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = IpcTransportError::endpoint_unavailable(endpoint, "Access is denied.");

        assert_eq!(
            format_daemon_ipc_error(&error),
            "failed to open daemon IPC endpoint at local-test. Another akraz-daemon may already be running, or the endpoint path may be unavailable. Details: Access is denied."
        );
    }

    #[test]
    fn daemon_ipc_error_reports_request_failure_detail() {
        let error = IpcTransportError::request_failed("pipe closed before a request line");

        assert_eq!(
            format_daemon_ipc_error(&error),
            "daemon IPC request failed. Details: pipe closed before a request line"
        );
    }
}
