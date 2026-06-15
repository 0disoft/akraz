use std::env;
use std::fmt::{Display, Formatter};
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use akraz_core::{
    CoreAction, DeviceId, EdgeCrossing, InjectedInputEvent, LogicalPoint, MouseButton, PeerId,
    PhysicalKey, PressState, RuntimeInputState, ScreenEdge, ScreenEdgeBinding, SessionId,
};
use akraz_daemon::{
    CoreActionDispatcher, DaemonInputCaptureConfig, DaemonInputCaptureWorker, DaemonIpcRunConfig,
    DaemonIpcServer, DaemonTransportCommand, LocalPlatformCoreActionDispatcher,
    LoopbackPeerTransport, ManagedPeerSessionTransport, PeerTransportCommandExecution,
    PeerTransportSession, PeerTransportSessionExecution, SharedCoreActionDispatcher,
    TcpPeerSessionTransport, TcpPeerTransport, TransportCoreActionDispatcher, build_daemon_status,
    execute_peer_transport_session_stream_until_closed, serve_daemon_ipc,
    serve_tcp_peer_transport_commands, serve_tcp_peer_transport_session,
    serve_tcp_peer_transport_session_and_execute, shared_runtime_state,
    start_daemon_input_capture_with_edge_bindings_and_dispatcher,
};
use akraz_identity::FileIdentityStore;
use akraz_ipc::{IpcEndpoint, IpcTransportError, resolve_current_default_endpoint};
use akraz_platform::{
    FakePlatformAdapter, PlatformAdapter, PlatformError, runtime_platform_adapter,
};
use serde::Serialize;

const PEER_LISTENER_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(25);

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
        Ok(DaemonCommand::PeerSessionSmoke) => run_peer_session_smoke(),
        Ok(DaemonCommand::PeerSessionExecutorSmoke) => run_peer_session_executor_smoke(),
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
    let endpoint = match options.endpoint.clone() {
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
    let local_device_id = match resolve_local_device_id(&options) {
        Ok(local_device_id) => local_device_id,
        Err(error) => {
            eprintln!("failed to resolve daemon identity: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (dispatcher, peer_sessions) = match build_configured_core_action_dispatcher(
        platform.clone(),
        &options,
        local_device_id,
    ) {
        Ok(dispatcher) => dispatcher,
        Err(error) => {
            eprintln!("failed to configure daemon recovery dispatcher: {error}");
            return ExitCode::FAILURE;
        }
    };
    let server = DaemonIpcServer::from_shared_state_dispatcher_and_peer_sessions(
        shared_runtime_state(RuntimeInputState::new()),
        platform.clone(),
        dispatcher.clone(),
        peer_sessions,
    );
    let peer_listener_worker = match options.peer_listen {
        Some(address) => match start_peer_session_listener(address, platform.clone()) {
            Ok(worker) => Some(worker),
            Err(error) => {
                eprintln!("failed to start peer session listener: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let capture_worker =
        match start_configured_input_capture(&server, &platform, &options, dispatcher) {
            Ok(worker) => worker,
            Err(error) => {
                eprintln!("failed to start daemon input capture: {error}");
                if stop_peer_session_listener(peer_listener_worker).is_err() {
                    eprintln!("failed to stop peer session listener after startup error");
                }
                return ExitCode::FAILURE;
            }
        };

    eprintln!("akraz-daemon listening at {}", config.endpoint());
    let result = match serve_daemon_ipc(&config, &server) {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("{}", format_daemon_ipc_error(&error));
            Err(())
        }
    };

    let capture_result = stop_capture_worker(capture_worker);
    let peer_listener_result = stop_peer_session_listener(peer_listener_worker);

    match (capture_result, peer_listener_result, result) {
        (Ok(()), Ok(()), Ok(())) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn build_configured_core_action_dispatcher<P>(
    platform: P,
    options: &ServeOptions,
    local_device_id: Option<DeviceId>,
) -> Result<(SharedCoreActionDispatcher, ManagedPeerSessionTransport), PlatformError>
where
    P: PlatformAdapter + Clone + Send + Sync + 'static,
{
    let peer_sessions = ManagedPeerSessionTransport::new();
    if let Some(peer_session) = &options.peer_session {
        let local_device_id = local_device_id.ok_or_else(|| {
            PlatformError::new(
                "peer session transport requires --local-device-id or --identity-store",
            )
        })?;
        let transport = TcpPeerSessionTransport::connect(
            peer_session.peer_id.clone(),
            local_device_id,
            peer_session.address,
        )?;
        peer_sessions.attach_session(transport)?;
    }
    let dispatcher: SharedCoreActionDispatcher = Arc::new(LocalPlatformCoreActionDispatcher::new(
        platform,
        TransportCoreActionDispatcher::new(peer_sessions.clone()),
    ));

    Ok((dispatcher, peer_sessions))
}

fn resolve_local_device_id(options: &ServeOptions) -> Result<Option<DeviceId>, PlatformError> {
    if let Some(local_device_id) = &options.local_device_id {
        return Ok(Some(local_device_id.clone()));
    }
    let Some(identity_store) = &options.identity_store else {
        return Ok(None);
    };

    let store = FileIdentityStore::new(identity_store);
    let display_name = options
        .identity_display_name
        .as_deref()
        .unwrap_or("Akraz Daemon");
    let identity = store.load_or_create(display_name).map_err(|error| {
        PlatformError::new(format!(
            "failed to load identity store {}: {error}",
            identity_store.display()
        ))
    })?;
    Ok(Some(DeviceId::new(identity.identity().device_id())))
}

fn start_configured_input_capture<P>(
    server: &DaemonIpcServer<P>,
    platform: &P,
    options: &ServeOptions,
    dispatcher: SharedCoreActionDispatcher,
) -> Result<Option<DaemonInputCaptureWorker>, PlatformError>
where
    P: PlatformAdapter,
{
    if !options.capture_input {
        return Ok(None);
    }

    let worker = start_daemon_input_capture_with_edge_bindings_and_dispatcher(
        server.shared_state(),
        platform,
        DaemonInputCaptureConfig::default(),
        options.edge_bindings.clone(),
        dispatcher,
    )?;

    Ok(Some(worker))
}

#[derive(Debug)]
struct PeerSessionListenerWorker {
    running: Arc<AtomicBool>,
    handle: JoinHandle<Result<(), PlatformError>>,
}

fn start_peer_session_listener<P>(
    address: SocketAddr,
    platform: P,
) -> Result<PeerSessionListenerWorker, PlatformError>
where
    P: PlatformAdapter + Send + Sync + 'static,
{
    let listener = TcpListener::bind(address).map_err(|error| {
        PlatformError::new(format!(
            "failed to bind peer session listener at {address}: {error}"
        ))
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        PlatformError::new(format!(
            "failed to configure peer session listener at {address}: {error}"
        ))
    })?;
    let address = listener.local_addr().map_err(|error| {
        PlatformError::new(format!(
            "failed to read peer session listener address for {address}: {error}"
        ))
    })?;
    let running = Arc::new(AtomicBool::new(true));
    let worker_running = Arc::clone(&running);
    let handle =
        thread::spawn(move || run_peer_session_listener(listener, platform, worker_running));

    eprintln!("akraz-daemon peer session listener at {address}");
    Ok(PeerSessionListenerWorker { running, handle })
}

fn run_peer_session_listener<P>(
    listener: TcpListener,
    platform: P,
    running: Arc<AtomicBool>,
) -> Result<(), PlatformError>
where
    P: PlatformAdapter,
{
    while running.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut reader = std::io::BufReader::new(stream);
                if let Err(error) =
                    execute_peer_transport_session_stream_until_closed(&mut reader, &platform)
                {
                    eprintln!("peer session ended with error: {error}");
                    platform.release_all()?;
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(PEER_LISTENER_IDLE_POLL_INTERVAL);
            }
            Err(error) => {
                return Err(PlatformError::new(format!(
                    "failed to accept peer session: {error}"
                )));
            }
        }
    }

    Ok(())
}

fn stop_peer_session_listener(worker: Option<PeerSessionListenerWorker>) -> Result<(), ()> {
    let Some(worker) = worker else {
        return Ok(());
    };

    worker.running.store(false, Ordering::Release);
    match worker.handle.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            eprintln!("failed to stop peer session listener: {error}");
            Err(())
        }
        Err(_) => {
            eprintln!("peer session listener thread panicked");
            Err(())
        }
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

fn run_peer_session_smoke() -> ExitCode {
    match build_peer_session_smoke_report() {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("failed to encode peer session smoke report: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("peer session smoke failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_peer_session_executor_smoke() -> ExitCode {
    match build_peer_session_executor_smoke_report() {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("failed to encode peer session executor smoke report: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("peer session executor smoke failed: {error}");
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

fn build_peer_session_smoke_report() -> Result<PeerSessionSmokeReport, akraz_platform::PlatformError>
{
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
        akraz_platform::PlatformError::new(format!(
            "failed to bind peer session smoke listener: {error}"
        ))
    })?;
    let address = listener.local_addr().map_err(|error| {
        akraz_platform::PlatformError::new(format!(
            "failed to read peer session smoke listener address: {error}"
        ))
    })?;
    let server_thread = thread::spawn(move || {
        serve_tcp_peer_transport_session(&listener, loopback_transport_smoke_actions().len())
    });
    let transport = TcpPeerSessionTransport::connect(
        PeerId::new("loopback-peer"),
        DeviceId::new("local-smoke-device"),
        address,
    )?;
    let dispatcher = TransportCoreActionDispatcher::new(transport);

    dispatcher.dispatch_core_actions(&loopback_transport_smoke_actions())?;

    let session = server_thread
        .join()
        .map_err(|_| akraz_platform::PlatformError::new("peer session smoke server panicked"))??;

    Ok(PeerSessionSmokeReport::from_session(
        env!("CARGO_PKG_VERSION"),
        &session,
    ))
}

fn build_peer_session_executor_smoke_report()
-> Result<PeerSessionExecutorSmokeReport, akraz_platform::PlatformError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
        akraz_platform::PlatformError::new(format!(
            "failed to bind peer session executor smoke listener: {error}"
        ))
    })?;
    let address = listener.local_addr().map_err(|error| {
        akraz_platform::PlatformError::new(format!(
            "failed to read peer session executor smoke listener address: {error}"
        ))
    })?;
    let server_thread = thread::spawn(move || {
        let platform = FakePlatformAdapter::default();
        let execution = serve_tcp_peer_transport_session_and_execute(
            &listener,
            loopback_transport_smoke_actions().len(),
            &platform,
        )?;
        let injected_events = platform.injected_events()?;
        let release_all_count = platform.release_all_count()?;

        Ok::<_, PlatformError>((execution, injected_events, release_all_count))
    });
    let transport = TcpPeerSessionTransport::connect(
        PeerId::new("loopback-peer"),
        DeviceId::new("local-smoke-device"),
        address,
    )?;
    let dispatcher = TransportCoreActionDispatcher::new(transport);

    dispatcher.dispatch_core_actions(&loopback_transport_smoke_actions())?;

    let (execution, injected_events, release_all_count) =
        server_thread.join().map_err(|_| {
            akraz_platform::PlatformError::new("peer session executor smoke server panicked")
        })??;

    Ok(PeerSessionExecutorSmokeReport::from_execution(
        env!("CARGO_PKG_VERSION"),
        &execution,
        &injected_events,
        release_all_count,
    ))
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
#[serde(rename_all = "camelCase")]
struct PeerSessionSmokeReport {
    daemon_version: &'static str,
    hello: PeerSessionSmokeHello,
    commands: Vec<LoopbackTransportSmokeCommand>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PeerSessionExecutorSmokeReport {
    daemon_version: &'static str,
    hello: PeerSessionSmokeHello,
    outcomes: Vec<PeerSessionExecutorSmokeOutcome>,
    injected_inputs: Vec<LoopbackTransportSmokeInputEvent>,
    release_all_count: u64,
}

impl PeerSessionSmokeReport {
    fn from_session(daemon_version: &'static str, session: &PeerTransportSession) -> Self {
        Self {
            daemon_version,
            hello: PeerSessionSmokeHello {
                protocol_major: session.hello.protocol.major,
                protocol_minor: session.hello.protocol.minor,
                device_id: session.hello.device_id.as_str().to_string(),
                peer_id: session.hello.peer_id.as_str().to_string(),
            },
            commands: session
                .commands
                .iter()
                .map(LoopbackTransportSmokeCommand::from)
                .collect(),
        }
    }
}

impl PeerSessionExecutorSmokeReport {
    fn from_execution(
        daemon_version: &'static str,
        execution: &PeerTransportSessionExecution,
        injected_events: &[InjectedInputEvent],
        release_all_count: u64,
    ) -> Self {
        Self {
            daemon_version,
            hello: PeerSessionSmokeHello {
                protocol_major: execution.hello.protocol.major,
                protocol_minor: execution.hello.protocol.minor,
                device_id: execution.hello.device_id.as_str().to_string(),
                peer_id: execution.hello.peer_id.as_str().to_string(),
            },
            outcomes: execution
                .outcomes
                .iter()
                .map(PeerSessionExecutorSmokeOutcome::from)
                .collect(),
            injected_inputs: injected_events
                .iter()
                .map(LoopbackTransportSmokeInputEvent::from)
                .collect(),
            release_all_count,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PeerSessionSmokeHello {
    protocol_major: u16,
    protocol_minor: u16,
    device_id: String,
    peer_id: String,
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

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum PeerSessionExecutorSmokeOutcome {
    RemoteSessionStarted {
        peer_id: String,
        crossing: Option<LoopbackTransportSmokeCrossing>,
    },
    InputForwarded {
        event: LoopbackTransportSmokeInputEvent,
    },
    InputsReleased,
    RemoteSessionStopped {
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

impl From<&PeerTransportCommandExecution> for PeerSessionExecutorSmokeOutcome {
    fn from(outcome: &PeerTransportCommandExecution) -> Self {
        match outcome {
            PeerTransportCommandExecution::RemoteSessionStarted { peer_id, crossing } => {
                Self::RemoteSessionStarted {
                    peer_id: peer_id.as_str().to_string(),
                    crossing: crossing.as_ref().map(LoopbackTransportSmokeCrossing::from),
                }
            }
            PeerTransportCommandExecution::InputForwarded { event } => Self::InputForwarded {
                event: LoopbackTransportSmokeInputEvent::from(event),
            },
            PeerTransportCommandExecution::InputsReleased => Self::InputsReleased,
            PeerTransportCommandExecution::RemoteSessionStopped { session_id } => {
                Self::RemoteSessionStopped {
                    session_id: session_id
                        .as_ref()
                        .map(|session_id| session_id.as_str().to_string()),
                }
            }
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
#[allow(clippy::large_enum_variant)]
enum DaemonCommand {
    Serve(ServeOptions),
    Status,
    Version,
    LoopbackTransportSmoke,
    TcpTransportSmoke,
    PeerSessionSmoke,
    PeerSessionExecutorSmoke,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ServeOptions {
    endpoint: Option<IpcEndpoint>,
    once: bool,
    capture_input: bool,
    edge_bindings: Vec<ScreenEdgeBinding>,
    peer_listen: Option<SocketAddr>,
    peer_session: Option<ServePeerSessionOptions>,
    local_device_id: Option<DeviceId>,
    identity_store: Option<PathBuf>,
    identity_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServePeerSessionOptions {
    peer_id: PeerId,
    address: SocketAddr,
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
        "--akraz-smoke-peer-session" => reject_trailing_args(args, DaemonCommand::PeerSessionSmoke),
        "--akraz-smoke-peer-session-executor" => {
            reject_trailing_args(args, DaemonCommand::PeerSessionExecutorSmoke)
        }
        "--serve" => parse_serve_options(args),
        argument
            if argument.starts_with("--endpoint")
                || argument == "--once"
                || argument == "--capture-input"
                || argument.starts_with("--edge-binding")
                || argument.starts_with("--peer-listen")
                || argument.starts_with("--peer-session")
                || argument.starts_with("--local-device-id")
                || argument.starts_with("--identity-store")
                || argument.starts_with("--identity-display-name") =>
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
        } else if let Some(value) = argument.strip_prefix("--peer-listen=") {
            set_peer_listen(&mut options, value)?;
        } else if argument == "--peer-listen" {
            let value = args
                .next()
                .ok_or(DaemonUsageError::MissingPeerListenValue)?;
            set_peer_listen(&mut options, &value)?;
        } else if let Some(value) = argument.strip_prefix("--peer-session=") {
            set_peer_session(&mut options, value)?;
        } else if argument == "--peer-session" {
            let value = args
                .next()
                .ok_or(DaemonUsageError::MissingPeerSessionValue)?;
            set_peer_session(&mut options, &value)?;
        } else if let Some(value) = argument.strip_prefix("--local-device-id=") {
            options.local_device_id = Some(parse_device_id(value)?);
        } else if argument == "--local-device-id" {
            let value = args
                .next()
                .ok_or(DaemonUsageError::MissingLocalDeviceIdValue)?;
            options.local_device_id = Some(parse_device_id(&value)?);
        } else if let Some(value) = argument.strip_prefix("--identity-store=") {
            options.identity_store = Some(parse_identity_store(value)?);
        } else if argument == "--identity-store" {
            let value = args
                .next()
                .ok_or(DaemonUsageError::MissingIdentityStoreValue)?;
            options.identity_store = Some(parse_identity_store(&value)?);
        } else if let Some(value) = argument.strip_prefix("--identity-display-name=") {
            options.identity_display_name = Some(parse_identity_display_name(value)?);
        } else if argument == "--identity-display-name" {
            let value = args
                .next()
                .ok_or(DaemonUsageError::MissingIdentityDisplayNameValue)?;
            options.identity_display_name = Some(parse_identity_display_name(&value)?);
        } else {
            return Err(DaemonUsageError::UnknownArgument(argument));
        }
    }
    if options.peer_session.is_some()
        && options.local_device_id.is_none()
        && options.identity_store.is_none()
    {
        return Err(DaemonUsageError::MissingLocalIdentityForPeerSession);
    }

    Ok(DaemonCommand::Serve(options))
}

fn set_peer_listen(options: &mut ServeOptions, value: &str) -> Result<(), DaemonUsageError> {
    if options.peer_listen.is_some() {
        return Err(DaemonUsageError::DuplicatePeerListen);
    }

    options.peer_listen = Some(parse_socket_address(
        value,
        DaemonUsageError::InvalidPeerListen,
    )?);
    Ok(())
}

fn set_peer_session(options: &mut ServeOptions, value: &str) -> Result<(), DaemonUsageError> {
    if options.peer_session.is_some() {
        return Err(DaemonUsageError::DuplicatePeerSession);
    }

    options.peer_session = Some(parse_peer_session(value)?);
    Ok(())
}

fn parse_peer_session(value: &str) -> Result<ServePeerSessionOptions, DaemonUsageError> {
    let Some((peer_id, address)) = value.rsplit_once('@') else {
        return Err(DaemonUsageError::InvalidPeerSession(value.to_string()));
    };
    let peer_id = peer_id.trim();
    if peer_id.is_empty() || peer_id.contains('@') {
        return Err(DaemonUsageError::InvalidPeerSession(value.to_string()));
    }

    Ok(ServePeerSessionOptions {
        peer_id: PeerId::new(peer_id),
        address: parse_socket_address(address, DaemonUsageError::InvalidPeerSession)?,
    })
}

fn parse_socket_address(
    value: &str,
    invalid: impl FnOnce(String) -> DaemonUsageError,
) -> Result<SocketAddr, DaemonUsageError> {
    value
        .trim()
        .parse::<SocketAddr>()
        .map_err(|_| invalid(value.to_string()))
}

fn parse_device_id(value: &str) -> Result<DeviceId, DaemonUsageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DaemonUsageError::InvalidLocalDeviceId(value.to_string()));
    }

    Ok(DeviceId::new(value))
}

fn parse_identity_store(value: &str) -> Result<PathBuf, DaemonUsageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DaemonUsageError::InvalidIdentityStore(value.to_string()));
    }

    Ok(PathBuf::from(value))
}

fn parse_identity_display_name(value: &str) -> Result<String, DaemonUsageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DaemonUsageError::InvalidIdentityDisplayName(
            value.to_string(),
        ));
    }

    Ok(value.to_string())
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
    MissingPeerListenValue,
    MissingPeerSessionValue,
    MissingLocalDeviceIdValue,
    MissingIdentityStoreValue,
    MissingIdentityDisplayNameValue,
    MissingLocalIdentityForPeerSession,
    DuplicatePeerListen,
    DuplicatePeerSession,
    InvalidEndpoint(String),
    InvalidEdgeBinding(String),
    InvalidPeerListen(String),
    InvalidPeerSession(String),
    InvalidLocalDeviceId(String),
    InvalidIdentityStore(String),
    InvalidIdentityDisplayName(String),
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
            Self::MissingPeerListenValue => formatter.write_str("missing value for --peer-listen"),
            Self::MissingPeerSessionValue => {
                formatter.write_str("missing value for --peer-session")
            }
            Self::MissingLocalDeviceIdValue => {
                formatter.write_str("missing value for --local-device-id")
            }
            Self::MissingIdentityStoreValue => {
                formatter.write_str("missing value for --identity-store")
            }
            Self::MissingIdentityDisplayNameValue => {
                formatter.write_str("missing value for --identity-display-name")
            }
            Self::MissingLocalIdentityForPeerSession => {
                formatter.write_str("--peer-session requires --local-device-id or --identity-store")
            }
            Self::DuplicatePeerListen => {
                formatter.write_str("--peer-listen can only be provided once")
            }
            Self::DuplicatePeerSession => {
                formatter.write_str("--peer-session can only be provided once")
            }
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
            Self::InvalidEdgeBinding(value) => write!(
                formatter,
                "invalid edge binding: {value}. Expected <local-edge>:<peer-id>:<remote-edge>"
            ),
            Self::InvalidPeerListen(value) => write!(
                formatter,
                "invalid peer listener address: {value}. Expected <ip>:<port> or [<ipv6>]:<port>"
            ),
            Self::InvalidPeerSession(value) => write!(
                formatter,
                "invalid peer session: {value}. Expected <peer-id>@<ip>:<port>"
            ),
            Self::InvalidLocalDeviceId(value) => {
                write!(formatter, "invalid local device id: {value}")
            }
            Self::InvalidIdentityStore(value) => write!(
                formatter,
                "invalid identity store path: {value}. Expected a non-empty path"
            ),
            Self::InvalidIdentityDisplayName(value) => write!(
                formatter,
                "invalid identity display name: {value}. Expected a non-empty display name"
            ),
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument: {argument}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf};

    use akraz_core::{DeviceId, PeerId, ScreenEdge, ScreenEdgeBinding};
    use akraz_ipc::{IpcEndpoint, IpcEndpointKind, IpcTransportError};

    use super::{
        DaemonCommand, DaemonUsageError, LoopbackTransportSmokeCommand,
        LoopbackTransportSmokeInputEvent, ServeOptions, ServePeerSessionOptions,
        build_loopback_transport_smoke_report, build_peer_session_executor_smoke_report,
        build_peer_session_smoke_report, build_tcp_transport_smoke_report, format_daemon_ipc_error,
        parse_daemon_command,
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
                peer_listen: None,
                peer_session: None,
                local_device_id: None,
                identity_store: None,
                identity_display_name: None,
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
                peer_listen: None,
                peer_session: None,
                local_device_id: None,
                identity_store: None,
                identity_display_name: None,
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
                peer_listen: None,
                peer_session: None,
                local_device_id: None,
                identity_store: None,
                identity_display_name: None,
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
                peer_listen: None,
                peer_session: None,
                local_device_id: None,
                identity_store: None,
                identity_display_name: None,
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
                peer_listen: None,
                peer_session: None,
                local_device_id: None,
                identity_store: None,
                identity_display_name: None,
            }))
        );
    }

    #[test]
    fn parses_manual_peer_session_serve_options() {
        assert_eq!(
            parse_daemon_command(
                [
                    "--serve",
                    "--peer-listen",
                    "127.0.0.1:24887",
                    "--local-device-id",
                    "windows-desktop",
                    "--peer-session",
                    "linux-laptop@127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
                capture_input: false,
                edge_bindings: Vec::new(),
                peer_listen: Some(
                    "127.0.0.1:24887"
                        .parse::<SocketAddr>()
                        .expect("peer listen address")
                ),
                peer_session: Some(ServePeerSessionOptions {
                    peer_id: PeerId::new("linux-laptop"),
                    address: "127.0.0.1:24888"
                        .parse::<SocketAddr>()
                        .expect("peer session address"),
                }),
                local_device_id: Some(DeviceId::new("windows-desktop")),
                identity_store: None,
                identity_display_name: None,
            }))
        );

        assert_eq!(
            parse_daemon_command(
                [
                    "--peer-listen=127.0.0.1:24887",
                    "--local-device-id=windows-desktop",
                    "--peer-session=linux-laptop@127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
                capture_input: false,
                edge_bindings: Vec::new(),
                peer_listen: Some(
                    "127.0.0.1:24887"
                        .parse::<SocketAddr>()
                        .expect("peer listen address")
                ),
                peer_session: Some(ServePeerSessionOptions {
                    peer_id: PeerId::new("linux-laptop"),
                    address: "127.0.0.1:24888"
                        .parse::<SocketAddr>()
                        .expect("peer session address"),
                }),
                local_device_id: Some(DeviceId::new("windows-desktop")),
                identity_store: None,
                identity_display_name: None,
            }))
        );
    }

    #[test]
    fn parses_identity_store_peer_session_serve_options() {
        assert_eq!(
            parse_daemon_command(
                [
                    "--identity-store",
                    "akraz-identity.json",
                    "--identity-display-name",
                    "Windows Desktop",
                    "--peer-session",
                    "linux-laptop@127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
                capture_input: false,
                edge_bindings: Vec::new(),
                peer_listen: None,
                peer_session: Some(ServePeerSessionOptions {
                    peer_id: PeerId::new("linux-laptop"),
                    address: "127.0.0.1:24888"
                        .parse::<SocketAddr>()
                        .expect("peer session address"),
                }),
                local_device_id: None,
                identity_store: Some(PathBuf::from("akraz-identity.json")),
                identity_display_name: Some("Windows Desktop".to_string()),
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
    fn parses_hidden_peer_session_smoke_command() {
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-peer-session"].map(String::from)),
            Ok(DaemonCommand::PeerSessionSmoke)
        );
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-peer-session", "--once"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--once".to_string()))
        );
    }

    #[test]
    fn parses_hidden_peer_session_executor_smoke_command() {
        assert_eq!(
            parse_daemon_command(["--akraz-smoke-peer-session-executor"].map(String::from)),
            Ok(DaemonCommand::PeerSessionExecutorSmoke)
        );
        assert_eq!(
            parse_daemon_command(
                ["--akraz-smoke-peer-session-executor", "--once"].map(String::from)
            ),
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
    fn peer_session_smoke_report_covers_hello_and_transport_commands() {
        let report = build_peer_session_smoke_report().expect("peer session smoke report");

        assert_eq!(report.daemon_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.hello.protocol_major, 1);
        assert_eq!(report.hello.protocol_minor, 0);
        assert_eq!(report.hello.device_id, "local-smoke-device");
        assert_eq!(report.hello.peer_id, "loopback-peer");
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
    fn peer_session_executor_smoke_report_covers_platform_execution() {
        let report =
            build_peer_session_executor_smoke_report().expect("peer session executor smoke report");

        assert_eq!(report.daemon_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.hello.protocol_major, 1);
        assert_eq!(report.hello.protocol_minor, 0);
        assert_eq!(report.hello.device_id, "local-smoke-device");
        assert_eq!(report.hello.peer_id, "loopback-peer");
        assert_eq!(report.outcomes.len(), 4);
        assert_eq!(report.injected_inputs.len(), 1);
        assert_eq!(
            report.injected_inputs[0],
            LoopbackTransportSmokeInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            }
        );
        assert_eq!(report.release_all_count, 1);
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
            parse_daemon_command(["--peer-listen"].map(String::from)),
            Err(DaemonUsageError::MissingPeerListenValue)
        );
        assert_eq!(
            parse_daemon_command(["--peer-session"].map(String::from)),
            Err(DaemonUsageError::MissingPeerSessionValue)
        );
        assert_eq!(
            parse_daemon_command(["--local-device-id"].map(String::from)),
            Err(DaemonUsageError::MissingLocalDeviceIdValue)
        );
        assert_eq!(
            parse_daemon_command(["--peer-listen", "not-an-address"].map(String::from)),
            Err(DaemonUsageError::InvalidPeerListen(
                "not-an-address".to_string()
            ))
        );
        assert_eq!(
            parse_daemon_command(
                [
                    "--local-device-id",
                    "windows-desktop",
                    "--peer-session",
                    "missing-address"
                ]
                .map(String::from)
            ),
            Err(DaemonUsageError::InvalidPeerSession(
                "missing-address".to_string()
            ))
        );
        assert_eq!(
            parse_daemon_command(
                ["--peer-session", "linux-laptop@127.0.0.1:24888"].map(String::from)
            ),
            Err(DaemonUsageError::MissingLocalIdentityForPeerSession)
        );
        assert_eq!(
            parse_daemon_command(
                [
                    "--peer-listen",
                    "127.0.0.1:24887",
                    "--peer-listen",
                    "127.0.0.1:24888"
                ]
                .map(String::from)
            ),
            Err(DaemonUsageError::DuplicatePeerListen)
        );
        assert_eq!(
            parse_daemon_command(
                [
                    "--local-device-id",
                    "windows-desktop",
                    "--peer-session",
                    "linux-laptop@127.0.0.1:24888",
                    "--peer-session",
                    "other-laptop@127.0.0.1:24889"
                ]
                .map(String::from)
            ),
            Err(DaemonUsageError::DuplicatePeerSession)
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
