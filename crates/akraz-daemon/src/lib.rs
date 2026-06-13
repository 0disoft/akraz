//! Daemon status builders shared by akraz daemon and diagnostic clients.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use akraz_core::{
    CapturedInputEvent, ControlMode, CoreAction, CoreTransitionError, EdgeCrossing,
    InjectedInputEvent, LogicalPoint, MouseButton, PeerId, PhysicalKey, PressState, RuntimeEvent,
    RuntimeInputState, ScreenEdge, ScreenEdgeBinding, ScreenLayout, SessionId,
};
use akraz_ipc::{
    DaemonStatus, IpcCodecError, IpcPlatformCapabilities, IpcRequest, IpcTransportError,
    JsonRpcError, JsonRpcFailure, JsonRpcSuccess, LocalIpcServer, PermissionIssue,
    PermissionsProbe, ProtocolVersionSnapshot, parse_request_line, serve_os_local_ipc_once,
    to_json_line,
};
use akraz_platform::{
    DesktopGeometry, InputCaptureConfig, InputCaptureSession, PlatformAdapter, PlatformError,
};
use akraz_protocol::ProtocolVersion;
use serde::{Deserialize, Serialize};

/// Current daemon package version.
pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

const JSONRPC_DAEMON_ERROR: i32 = -32000;
const DEFAULT_CAPTURE_DRAIN_BATCH_SIZE: usize = 64;
const DEFAULT_CAPTURE_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Shared daemon runtime state observed by IPC and capture workers.
pub type SharedRuntimeInputState = Arc<Mutex<RuntimeInputState>>;

/// Dispatches side effects requested by the core input state machine.
pub trait CoreActionDispatcher: Send + Sync + 'static {
    fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError>;
}

/// No-op dispatcher used until a real peer transport is attached.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopCoreActionDispatcher;

impl CoreActionDispatcher for NoopCoreActionDispatcher {
    fn dispatch_core_actions(&self, _actions: &[CoreAction]) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// Transport-facing command derived from a core side-effect action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonTransportCommand {
    StartRemoteSession {
        peer_id: PeerId,
        crossing: Option<EdgeCrossing>,
    },
    ForwardInput {
        event: InjectedInputEvent,
    },
    ReleaseAllInputs,
    StopRemoteSession {
        session_id: Option<SessionId>,
    },
}

impl From<&CoreAction> for DaemonTransportCommand {
    fn from(action: &CoreAction) -> Self {
        match action {
            CoreAction::StartRemoteSession { peer_id, crossing } => Self::StartRemoteSession {
                peer_id: peer_id.clone(),
                crossing: crossing.clone(),
            },
            CoreAction::ForwardInput { event } => Self::ForwardInput {
                event: event.clone(),
            },
            CoreAction::ReleaseAllInputs => Self::ReleaseAllInputs,
            CoreAction::StopRemoteSession { session_id } => Self::StopRemoteSession {
                session_id: session_id.clone(),
            },
        }
    }
}

/// Peer transport boundary used by the daemon imperative shell.
pub trait DaemonPeerTransport: Send + Sync + 'static {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError>;
}

/// Core action dispatcher that maps core effects into peer transport commands.
#[derive(Debug, Clone)]
pub struct TransportCoreActionDispatcher<T> {
    transport: T,
}

impl<T> TransportCoreActionDispatcher<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T> CoreActionDispatcher for TransportCoreActionDispatcher<T>
where
    T: DaemonPeerTransport,
{
    fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError> {
        for action in actions {
            self.transport
                .dispatch_transport_command(DaemonTransportCommand::from(action))?;
        }

        Ok(())
    }
}

/// In-memory transport implementation used for deterministic local smoke tests.
#[derive(Debug, Default, Clone)]
pub struct LoopbackPeerTransport {
    commands: Arc<Mutex<Vec<DaemonTransportCommand>>>,
}

impl LoopbackPeerTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Result<Vec<DaemonTransportCommand>, PlatformError> {
        self.commands
            .lock()
            .map_err(|_| PlatformError::new("loopback transport command log is unavailable"))
            .map(|commands| commands.clone())
    }
}

impl DaemonPeerTransport for LoopbackPeerTransport {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError> {
        self.commands
            .lock()
            .map_err(|_| PlatformError::new("loopback transport command log is unavailable"))?
            .push(command);

        Ok(())
    }
}

/// TCP peer transport that sends one newline-delimited JSON command per connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpPeerTransport {
    peer_id: PeerId,
    address: SocketAddr,
}

impl TcpPeerTransport {
    /// Create a TCP peer transport for one configured peer.
    pub fn new(peer_id: PeerId, address: SocketAddr) -> Self {
        Self { peer_id, address }
    }

    /// Return the peer this transport is configured to reach.
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Return the TCP address this transport connects to.
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl DaemonPeerTransport for TcpPeerTransport {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError> {
        validate_transport_command_peer(&self.peer_id, &command)?;

        let message = PeerTransportMessage::from_command(&command);
        let line = serde_json::to_string(&message).map_err(|error| {
            PlatformError::new(format!("failed to encode peer command: {error}"))
        })?;
        let mut stream = TcpStream::connect(self.address).map_err(|error| {
            PlatformError::new(format!(
                "failed to connect peer transport {} at {}: {error}",
                self.peer_id.as_str(),
                self.address
            ))
        })?;

        stream
            .write_all(line.as_bytes())
            .and_then(|()| stream.write_all(b"\n"))
            .and_then(|()| stream.flush())
            .map_err(|error| {
                PlatformError::new(format!(
                    "failed to send peer transport command to {} at {}: {error}",
                    self.peer_id.as_str(),
                    self.address
                ))
            })
    }
}

/// Serve a bounded number of TCP peer transport commands and return decoded commands.
pub fn serve_tcp_peer_transport_commands(
    listener: &TcpListener,
    max_commands: usize,
) -> Result<Vec<DaemonTransportCommand>, PlatformError> {
    let mut commands = Vec::with_capacity(max_commands);

    for _ in 0..max_commands {
        let (stream, _) = listener.accept().map_err(|error| {
            PlatformError::new(format!("failed to accept peer command: {error}"))
        })?;
        commands.push(read_tcp_peer_transport_command(stream)?);
    }

    Ok(commands)
}

fn read_tcp_peer_transport_command(
    stream: TcpStream,
) -> Result<DaemonTransportCommand, PlatformError> {
    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    let read = reader
        .read_line(&mut line)
        .map_err(|error| PlatformError::new(format!("failed to read peer command: {error}")))?;
    if read == 0 {
        return Err(PlatformError::new(
            "peer transport connection closed before a command line was received",
        ));
    }

    let message: PeerTransportMessage = serde_json::from_str(&line)
        .map_err(|error| PlatformError::new(format!("failed to decode peer command: {error}")))?;

    message.into_command()
}

fn validate_transport_command_peer(
    configured_peer_id: &PeerId,
    command: &DaemonTransportCommand,
) -> Result<(), PlatformError> {
    match command {
        DaemonTransportCommand::StartRemoteSession { peer_id, .. }
            if peer_id != configured_peer_id =>
        {
            Err(PlatformError::new(format!(
                "peer transport configured for {}, got start command for {}",
                configured_peer_id.as_str(),
                peer_id.as_str()
            )))
        }
        _ => Ok(()),
    }
}

/// Wire message exchanged by peer transport adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerTransportMessage {
    pub protocol: PeerTransportProtocolVersion,
    pub command: PeerTransportCommandPayload,
}

impl PeerTransportMessage {
    /// Build a wire message from an internal daemon transport command.
    pub fn from_command(command: &DaemonTransportCommand) -> Self {
        Self {
            protocol: PeerTransportProtocolVersion::current(),
            command: PeerTransportCommandPayload::from(command),
        }
    }

    /// Decode a wire message into an internal daemon transport command.
    pub fn into_command(self) -> Result<DaemonTransportCommand, PlatformError> {
        let version = ProtocolVersion {
            major: self.protocol.major,
            minor: self.protocol.minor,
        };
        if !ProtocolVersion::CURRENT.is_compatible_with(version) {
            return Err(PlatformError::new(format!(
                "unsupported peer transport protocol {}.{}",
                self.protocol.major, self.protocol.minor
            )));
        }

        self.command.into_command()
    }
}

/// Wire-safe protocol version for peer transport messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerTransportProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl PeerTransportProtocolVersion {
    fn current() -> Self {
        Self {
            major: ProtocolVersion::CURRENT.major,
            minor: ProtocolVersion::CURRENT.minor,
        }
    }
}

/// Wire-safe peer transport command payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PeerTransportCommandPayload {
    StartRemoteSession {
        peer_id: String,
        crossing: Option<PeerTransportCrossing>,
    },
    ForwardInput {
        event: PeerTransportInputEvent,
    },
    ReleaseAllInputs,
    StopRemoteSession {
        session_id: Option<String>,
    },
}

impl From<&DaemonTransportCommand> for PeerTransportCommandPayload {
    fn from(command: &DaemonTransportCommand) -> Self {
        match command {
            DaemonTransportCommand::StartRemoteSession { peer_id, crossing } => {
                Self::StartRemoteSession {
                    peer_id: peer_id.as_str().to_string(),
                    crossing: crossing.as_ref().map(PeerTransportCrossing::from),
                }
            }
            DaemonTransportCommand::ForwardInput { event } => Self::ForwardInput {
                event: PeerTransportInputEvent::from(event),
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

impl PeerTransportCommandPayload {
    fn into_command(self) -> Result<DaemonTransportCommand, PlatformError> {
        match self {
            Self::StartRemoteSession { peer_id, crossing } => {
                let crossing = match crossing {
                    Some(crossing) => Some(crossing.into_crossing()?),
                    None => None,
                };

                Ok(DaemonTransportCommand::StartRemoteSession {
                    peer_id: PeerId::new(peer_id),
                    crossing,
                })
            }
            Self::ForwardInput { event } => Ok(DaemonTransportCommand::ForwardInput {
                event: event.into_input_event()?,
            }),
            Self::ReleaseAllInputs => Ok(DaemonTransportCommand::ReleaseAllInputs),
            Self::StopRemoteSession { session_id } => {
                Ok(DaemonTransportCommand::StopRemoteSession {
                    session_id: session_id.map(SessionId::new),
                })
            }
        }
    }
}

/// Wire-safe edge crossing payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerTransportCrossing {
    pub peer_id: String,
    pub local_edge: String,
    pub remote_edge: String,
    pub exit_x: i32,
    pub exit_y: i32,
    pub edge_offset: i32,
}

impl From<&EdgeCrossing> for PeerTransportCrossing {
    fn from(crossing: &EdgeCrossing) -> Self {
        Self {
            peer_id: crossing.peer_id.as_str().to_string(),
            local_edge: screen_edge_name(crossing.local_edge).to_string(),
            remote_edge: screen_edge_name(crossing.remote_edge).to_string(),
            exit_x: crossing.exit_position.x,
            exit_y: crossing.exit_position.y,
            edge_offset: crossing.edge_offset,
        }
    }
}

impl PeerTransportCrossing {
    fn into_crossing(self) -> Result<EdgeCrossing, PlatformError> {
        Ok(EdgeCrossing {
            peer_id: PeerId::new(self.peer_id),
            local_edge: parse_screen_edge_name(&self.local_edge)?,
            remote_edge: parse_screen_edge_name(&self.remote_edge)?,
            exit_position: LogicalPoint {
                x: self.exit_x,
                y: self.exit_y,
            },
            edge_offset: self.edge_offset,
        })
    }
}

/// Wire-safe input event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PeerTransportInputEvent {
    Key { key: String, state: String },
    MouseButton { button: String, state: String },
    PointerMoved { delta_x: i32, delta_y: i32 },
}

impl From<&InjectedInputEvent> for PeerTransportInputEvent {
    fn from(event: &InjectedInputEvent) -> Self {
        match event {
            InjectedInputEvent::Key { key, state } => Self::Key {
                key: physical_key_name(*key),
                state: press_state_name(*state).to_string(),
            },
            InjectedInputEvent::MouseButton { button, state } => Self::MouseButton {
                button: mouse_button_name(*button),
                state: press_state_name(*state).to_string(),
            },
            InjectedInputEvent::PointerMoved { delta_x, delta_y } => Self::PointerMoved {
                delta_x: *delta_x,
                delta_y: *delta_y,
            },
        }
    }
}

impl PeerTransportInputEvent {
    fn into_input_event(self) -> Result<InjectedInputEvent, PlatformError> {
        match self {
            Self::Key { key, state } => Ok(InjectedInputEvent::Key {
                key: parse_physical_key_name(&key)?,
                state: parse_press_state_name(&state)?,
            }),
            Self::MouseButton { button, state } => Ok(InjectedInputEvent::MouseButton {
                button: parse_mouse_button_name(&button)?,
                state: parse_press_state_name(&state)?,
            }),
            Self::PointerMoved { delta_x, delta_y } => {
                Ok(InjectedInputEvent::PointerMoved { delta_x, delta_y })
            }
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

fn parse_screen_edge_name(value: &str) -> Result<ScreenEdge, PlatformError> {
    match value {
        "left" => Ok(ScreenEdge::Left),
        "right" => Ok(ScreenEdge::Right),
        "top" => Ok(ScreenEdge::Top),
        "bottom" => Ok(ScreenEdge::Bottom),
        _ => Err(PlatformError::new(format!(
            "unsupported peer transport screen edge: {value}"
        ))),
    }
}

fn press_state_name(state: PressState) -> &'static str {
    match state {
        PressState::Pressed => "pressed",
        PressState::Released => "released",
    }
}

fn parse_press_state_name(value: &str) -> Result<PressState, PlatformError> {
    match value {
        "pressed" => Ok(PressState::Pressed),
        "released" => Ok(PressState::Released),
        _ => Err(PlatformError::new(format!(
            "unsupported peer transport press state: {value}"
        ))),
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

fn parse_physical_key_name(value: &str) -> Result<PhysicalKey, PlatformError> {
    match value {
        "leftShift" => Ok(PhysicalKey::LeftShift),
        "rightShift" => Ok(PhysicalKey::RightShift),
        "leftControl" => Ok(PhysicalKey::LeftControl),
        "rightControl" => Ok(PhysicalKey::RightControl),
        "leftAlt" => Ok(PhysicalKey::LeftAlt),
        "rightAlt" => Ok(PhysicalKey::RightAlt),
        "leftMeta" => Ok(PhysicalKey::LeftMeta),
        "rightMeta" => Ok(PhysicalKey::RightMeta),
        value => parse_prefixed_u16(value, "code:").map(PhysicalKey::Code),
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

fn parse_mouse_button_name(value: &str) -> Result<MouseButton, PlatformError> {
    match value {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        "back" => Ok(MouseButton::Back),
        "forward" => Ok(MouseButton::Forward),
        value => parse_prefixed_u16(value, "other:").map(MouseButton::Other),
    }
}

fn parse_prefixed_u16(value: &str, prefix: &str) -> Result<u16, PlatformError> {
    let Some(raw_code) = value.strip_prefix(prefix) else {
        return Err(PlatformError::new(format!(
            "unsupported peer transport value: {value}"
        )));
    };

    raw_code.parse::<u16>().map_err(|error| {
        PlatformError::new(format!(
            "invalid peer transport numeric code {raw_code}: {error}"
        ))
    })
}

/// Configuration used to translate captured pointer deltas into core routing events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonInputRoutingConfig {
    pub screen_layout: ScreenLayout,
    pub initial_pointer: LogicalPoint,
}

impl DaemonInputRoutingConfig {
    /// Build routing config from platform desktop geometry and configured peer edge bindings.
    pub fn from_desktop_geometry(
        geometry: DesktopGeometry,
        edge_bindings: Vec<ScreenEdgeBinding>,
    ) -> Self {
        Self {
            screen_layout: ScreenLayout::new(geometry.virtual_screen_bounds, edge_bindings),
            initial_pointer: geometry.pointer_position,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedInputRouter {
    screen_layout: ScreenLayout,
    current_pointer: LogicalPoint,
}

impl CapturedInputRouter {
    fn new(config: DaemonInputRoutingConfig) -> Self {
        Self {
            screen_layout: config.screen_layout,
            current_pointer: config.initial_pointer,
        }
    }
}

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
    state: SharedRuntimeInputState,
    platform: P,
}

impl<P> DaemonIpcServer<P> {
    /// Create an in-process daemon IPC server.
    pub fn new(state: RuntimeInputState, platform: P) -> Self {
        Self::from_shared_state(shared_runtime_state(state), platform)
    }

    /// Create an in-process daemon IPC server from shared runtime state.
    pub fn from_shared_state(state: SharedRuntimeInputState, platform: P) -> Self {
        Self { state, platform }
    }

    /// Return the runtime state shared by daemon background workers.
    pub fn shared_state(&self) -> SharedRuntimeInputState {
        Arc::clone(&self.state)
    }
}

impl<P> LocalIpcServer for DaemonIpcServer<P>
where
    P: PlatformAdapter,
{
    fn handle_request_line(&self, request_line: &str) -> Result<String, IpcTransportError> {
        let state = self.state.lock().map_err(|_| {
            IpcTransportError::request_failed("daemon runtime state is unavailable")
        })?;

        handle_ipc_request_line(&state, &self.platform, request_line)
            .map_err(|error| IpcTransportError::request_failed(error.to_string()))
    }
}

/// Build daemon runtime state shared by IPC and background workers.
pub fn shared_runtime_state(state: RuntimeInputState) -> SharedRuntimeInputState {
    Arc::new(Mutex::new(state))
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

/// Drain captured input events into the core runtime state without blocking.
pub fn drain_capture_events(
    state: &mut RuntimeInputState,
    capture: &InputCaptureSession,
    max_events: usize,
) -> Result<Vec<CoreAction>, CoreTransitionError> {
    let mut actions = Vec::new();

    for _ in 0..max_events {
        match capture.try_recv() {
            Ok(event) => actions.extend(state.apply_event(RuntimeEvent::Input(event))?),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }

    Ok(actions)
}

/// Configuration for the daemon capture worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonInputCaptureConfig {
    pub input_capture: InputCaptureConfig,
    pub drain_batch_size: usize,
    pub idle_poll_interval: Duration,
}

impl DaemonInputCaptureConfig {
    fn bounded_drain_batch_size(self) -> usize {
        self.drain_batch_size.max(1)
    }

    fn bounded_idle_poll_interval(self) -> Duration {
        if self.idle_poll_interval.is_zero() {
            DEFAULT_CAPTURE_IDLE_POLL_INTERVAL
        } else {
            self.idle_poll_interval
        }
    }
}

impl Default for DaemonInputCaptureConfig {
    fn default() -> Self {
        Self {
            input_capture: InputCaptureConfig::default(),
            drain_batch_size: DEFAULT_CAPTURE_DRAIN_BATCH_SIZE,
            idle_poll_interval: DEFAULT_CAPTURE_IDLE_POLL_INTERVAL,
        }
    }
}

/// Background worker that drains captured platform input into daemon runtime state.
pub struct DaemonInputCaptureWorker {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), PlatformError>>>,
}

impl DaemonInputCaptureWorker {
    fn new(running: Arc<AtomicBool>, thread: JoinHandle<Result<(), PlatformError>>) -> Self {
        Self {
            running,
            thread: Some(thread),
        }
    }

    /// Stop the capture worker and wait for its resources to exit.
    pub fn stop(mut self) -> Result<(), PlatformError> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<(), PlatformError> {
        self.running.store(false, Ordering::Release);

        let Some(thread) = self.thread.take() else {
            return Ok(());
        };

        match thread.join() {
            Ok(result) => result,
            Err(_) => Err(PlatformError::new("daemon input capture worker panicked")),
        }
    }
}

impl Drop for DaemonInputCaptureWorker {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

/// Start daemon input capture and continuously drain events into shared runtime state.
pub fn start_daemon_input_capture(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
) -> Result<DaemonInputCaptureWorker, PlatformError> {
    start_daemon_input_capture_with_edge_bindings(state, platform, config, Vec::new())
}

/// Start daemon input capture using platform geometry and configured edge bindings.
pub fn start_daemon_input_capture_with_edge_bindings(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    edge_bindings: Vec<ScreenEdgeBinding>,
) -> Result<DaemonInputCaptureWorker, PlatformError> {
    start_daemon_input_capture_with_edge_bindings_and_dispatcher(
        state,
        platform,
        config,
        edge_bindings,
        NoopCoreActionDispatcher,
    )
}

/// Start daemon input capture using platform geometry, configured edge bindings, and dispatcher.
pub fn start_daemon_input_capture_with_edge_bindings_and_dispatcher<D>(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    edge_bindings: Vec<ScreenEdgeBinding>,
    dispatcher: D,
) -> Result<DaemonInputCaptureWorker, PlatformError>
where
    D: CoreActionDispatcher,
{
    let geometry = platform.read_desktop_geometry()?;

    start_daemon_input_capture_with_routing_and_dispatcher(
        state,
        platform,
        config,
        DaemonInputRoutingConfig::from_desktop_geometry(geometry, edge_bindings),
        dispatcher,
    )
}

/// Start daemon input capture with explicit pointer routing configuration.
pub fn start_daemon_input_capture_with_routing(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
) -> Result<DaemonInputCaptureWorker, PlatformError> {
    start_daemon_input_capture_with_routing_and_dispatcher(
        state,
        platform,
        config,
        routing,
        NoopCoreActionDispatcher,
    )
}

/// Start daemon input capture with explicit routing and side-effect dispatch.
pub fn start_daemon_input_capture_with_routing_and_dispatcher<D>(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
    dispatcher: D,
) -> Result<DaemonInputCaptureWorker, PlatformError>
where
    D: CoreActionDispatcher,
{
    let capture = platform.start_input_capture(config.input_capture)?;
    let running = Arc::new(AtomicBool::new(true));
    let worker_running = Arc::clone(&running);
    let thread = thread::Builder::new()
        .name("akraz-daemon-input-capture".to_string())
        .spawn(move || {
            run_daemon_input_capture_worker(
                state,
                capture,
                worker_running,
                config,
                routing,
                dispatcher,
            )
        })
        .map_err(|error| {
            PlatformError::new(format!(
                "failed to start daemon input capture worker: {error}"
            ))
        })?;

    Ok(DaemonInputCaptureWorker::new(running, thread))
}

fn run_daemon_input_capture_worker<D>(
    state: SharedRuntimeInputState,
    capture: InputCaptureSession,
    running: Arc<AtomicBool>,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
    dispatcher: D,
) -> Result<(), PlatformError>
where
    D: CoreActionDispatcher,
{
    let idle_poll_interval = config.bounded_idle_poll_interval();
    let mut router = CapturedInputRouter::new(routing);

    while running.load(Ordering::Acquire) {
        match capture.recv_timeout(idle_poll_interval) {
            Ok(event) => {
                dispatch_core_action_batch(
                    &dispatcher,
                    apply_routed_capture_event(&state, &mut router, event)?,
                )?;
                dispatch_core_action_batch(
                    &dispatcher,
                    drain_ready_capture_events(
                        &state,
                        &capture,
                        &mut router,
                        config.bounded_drain_batch_size(),
                    )?,
                )?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }

    capture.stop()
}

fn dispatch_core_action_batch(
    dispatcher: &impl CoreActionDispatcher,
    actions: Vec<CoreAction>,
) -> Result<(), PlatformError> {
    if actions.is_empty() {
        return Ok(());
    }

    dispatcher.dispatch_core_actions(&actions)
}

fn drain_ready_capture_events(
    state: &SharedRuntimeInputState,
    capture: &InputCaptureSession,
    router: &mut CapturedInputRouter,
    max_events: usize,
) -> Result<Vec<CoreAction>, PlatformError> {
    let mut actions = Vec::new();

    for _ in 0..max_events {
        match capture.try_recv() {
            Ok(event) => actions.extend(apply_routed_capture_event(state, router, event)?),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }

    Ok(actions)
}

fn apply_routed_capture_event(
    state: &SharedRuntimeInputState,
    router: &mut CapturedInputRouter,
    event: CapturedInputEvent,
) -> Result<Vec<CoreAction>, PlatformError> {
    let mut state = state
        .lock()
        .map_err(|_| PlatformError::new("daemon runtime state is unavailable"))?;

    apply_routed_capture_event_to_state(&mut state, router, event).map_err(capture_transition_error)
}

fn apply_routed_capture_event_to_state(
    state: &mut RuntimeInputState,
    router: &mut CapturedInputRouter,
    event: CapturedInputEvent,
) -> Result<Vec<CoreAction>, CoreTransitionError> {
    match event {
        CapturedInputEvent::PointerMoved { delta_x, delta_y }
            if state.mode() == ControlMode::Local =>
        {
            let previous = router.current_pointer;
            let next = LogicalPoint {
                x: previous.x.saturating_add(delta_x),
                y: previous.y.saturating_add(delta_y),
            };
            router.current_pointer = next;

            state.apply_event(RuntimeEvent::LocalPointerMoved {
                previous,
                next,
                layout: router.screen_layout.clone(),
            })
        }
        event => state.apply_event(RuntimeEvent::Input(event)),
    }
}

fn capture_transition_error(error: CoreTransitionError) -> PlatformError {
    PlatformError::new(format!("failed to apply captured input: {error}"))
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
    use akraz_core::{
        CapturedInputEvent, ControlMode, CoreAction, EdgeCrossing, InjectedInputEvent,
        LogicalPoint, LogicalRect, LogicalSize, MouseButton, PeerId, PhysicalKey, PressState,
        RuntimeEvent, RuntimeInputState, ScreenEdge, ScreenEdgeBinding, ScreenLayout, SessionId,
    };
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, DaemonStatusParams, IpcEndpoint,
        IpcPlatformCapabilities, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcServer,
        METHOD_DAEMON_STATUS, OsLocalIpcClient, call_json_rpc, to_json_line,
    };
    use akraz_platform::{
        DesktopGeometry, FakePlatformAdapter, InputCaptureConfig, PlatformAdapter,
        PlatformCapabilities, PlatformError,
    };
    use serde_json::json;

    use super::{
        CapturedInputRouter, CoreActionDispatcher, DAEMON_VERSION, DaemonInputCaptureConfig,
        DaemonInputRoutingConfig, DaemonIpcRunConfig, DaemonIpcServer, DaemonPeerTransport,
        DaemonTransportCommand, LoopbackPeerTransport, PeerTransportMessage, TcpPeerTransport,
        TransportCoreActionDispatcher, apply_routed_capture_event_to_state, build_daemon_status,
        build_permissions_probe, drain_capture_events, handle_ipc_request_line, serve_daemon_ipc,
        serve_tcp_peer_transport_commands, shared_runtime_state, start_daemon_input_capture,
        start_daemon_input_capture_with_edge_bindings, start_daemon_input_capture_with_routing,
        start_daemon_input_capture_with_routing_and_dispatcher,
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

    fn right_edge_layout() -> ScreenLayout {
        ScreenLayout::new(
            LogicalRect {
                origin: LogicalPoint { x: 0, y: 0 },
                size: LogicalSize {
                    width: 1920,
                    height: 1080,
                },
            },
            vec![ScreenEdgeBinding {
                local_edge: ScreenEdge::Right,
                peer_id: PeerId::new("right-peer"),
                remote_edge: ScreenEdge::Left,
            }],
        )
    }

    fn desktop_geometry_at_right_edge() -> DesktopGeometry {
        DesktopGeometry {
            pointer_position: LogicalPoint { x: 1919, y: 540 },
            virtual_screen_bounds: LogicalRect {
                origin: LogicalPoint { x: 0, y: 0 },
                size: LogicalSize {
                    width: 1920,
                    height: 1080,
                },
            },
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

    #[derive(Debug, Default, Clone)]
    struct RecordingCoreActionDispatcher {
        actions: std::sync::Arc<std::sync::Mutex<Vec<CoreAction>>>,
    }

    impl RecordingCoreActionDispatcher {
        fn snapshot(&self) -> Vec<CoreAction> {
            self.actions.lock().expect("recorded action lock").clone()
        }
    }

    impl CoreActionDispatcher for RecordingCoreActionDispatcher {
        fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError> {
            self.actions
                .lock()
                .map_err(|_| PlatformError::new("recorded action lock was poisoned"))?
                .extend_from_slice(actions);

            Ok(())
        }
    }

    #[derive(Debug, Clone)]
    struct FailingPeerTransport {
        message: &'static str,
    }

    impl DaemonPeerTransport for FailingPeerTransport {
        fn dispatch_transport_command(
            &self,
            _command: DaemonTransportCommand,
        ) -> Result<(), PlatformError> {
            Err(PlatformError::new(self.message))
        }
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
    fn drain_capture_events_applies_bounded_input_to_core_state() {
        let press_platform =
            FakePlatformAdapter::default().with_captured_events(vec![CapturedInputEvent::Key {
                key: PhysicalKey::LeftShift,
                state: PressState::Pressed,
            }]);
        let release_platform =
            FakePlatformAdapter::default().with_captured_events(vec![CapturedInputEvent::Key {
                key: PhysicalKey::LeftShift,
                state: PressState::Released,
            }]);
        let press_capture = press_platform
            .start_input_capture(InputCaptureConfig::default())
            .expect("fake press capture session");
        let release_capture = release_platform
            .start_input_capture(InputCaptureConfig::default())
            .expect("fake release capture session");
        let mut state = RuntimeInputState::new();

        let first_actions =
            drain_capture_events(&mut state, &press_capture, 1).expect("first capture drain");

        assert!(first_actions.is_empty());
        assert!(state.pressed_keys().contains(&PhysicalKey::LeftShift));
        assert!(state.modifiers().left_shift);

        let second_actions =
            drain_capture_events(&mut state, &release_capture, 1).expect("second capture drain");

        assert!(second_actions.is_empty());
        assert!(!state.pressed_keys().contains(&PhysicalKey::LeftShift));
        assert!(!state.modifiers().left_shift);
    }

    #[test]
    fn routed_pointer_delta_updates_local_pointer_without_edge_crossing() {
        let mut state = RuntimeInputState::new();
        let mut router = CapturedInputRouter::new(DaemonInputRoutingConfig {
            screen_layout: right_edge_layout(),
            initial_pointer: LogicalPoint { x: 100, y: 100 },
        });

        let actions = apply_routed_capture_event_to_state(
            &mut state,
            &mut router,
            CapturedInputEvent::PointerMoved {
                delta_x: 5,
                delta_y: -3,
            },
        )
        .expect("routed pointer move");

        assert!(actions.is_empty());
        assert_eq!(state.mode(), ControlMode::Local);
        assert_eq!(
            state.last_local_pointer(),
            Some(LogicalPoint { x: 105, y: 97 })
        );
    }

    #[test]
    fn routing_config_uses_platform_geometry_and_configured_edges() {
        let config = DaemonInputRoutingConfig::from_desktop_geometry(
            desktop_geometry_at_right_edge(),
            vec![ScreenEdgeBinding {
                local_edge: ScreenEdge::Right,
                peer_id: PeerId::new("right-peer"),
                remote_edge: ScreenEdge::Left,
            }],
        );

        assert_eq!(config.initial_pointer, LogicalPoint { x: 1919, y: 540 });
        assert_eq!(config.screen_layout, right_edge_layout());
    }

    #[test]
    fn routed_pointer_delta_requests_remote_entry_on_edge_crossing() {
        let mut state = RuntimeInputState::new();
        let mut router = CapturedInputRouter::new(DaemonInputRoutingConfig {
            screen_layout: right_edge_layout(),
            initial_pointer: LogicalPoint { x: 1919, y: 540 },
        });

        let actions = apply_routed_capture_event_to_state(
            &mut state,
            &mut router,
            CapturedInputEvent::PointerMoved {
                delta_x: 1,
                delta_y: 0,
            },
        )
        .expect("routed edge crossing");

        assert_eq!(state.mode(), ControlMode::EnteringRemote);
        assert_eq!(state.pending_peer_id(), Some(&PeerId::new("right-peer")));
        assert_eq!(
            state.last_local_pointer(),
            Some(LogicalPoint { x: 1920, y: 540 })
        );
        assert_eq!(
            actions,
            vec![CoreAction::StartRemoteSession {
                peer_id: PeerId::new("right-peer"),
                crossing: Some(EdgeCrossing {
                    peer_id: PeerId::new("right-peer"),
                    local_edge: ScreenEdge::Right,
                    remote_edge: ScreenEdge::Left,
                    exit_position: LogicalPoint { x: 1920, y: 540 },
                    edge_offset: 540,
                }),
            }]
        );
    }

    #[test]
    fn routed_pointer_delta_remains_forwardable_while_remote() {
        let mut state = RuntimeInputState::new();
        state
            .apply_event(akraz_core::RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        state
            .apply_event(akraz_core::RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-1"),
            })
            .expect("remote entry confirmed");
        let mut router = CapturedInputRouter::new(DaemonInputRoutingConfig {
            screen_layout: right_edge_layout(),
            initial_pointer: LogicalPoint { x: 1919, y: 540 },
        });

        let actions = apply_routed_capture_event_to_state(
            &mut state,
            &mut router,
            CapturedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            },
        )
        .expect("routed remote pointer move");

        assert_eq!(
            actions,
            vec![CoreAction::ForwardInput {
                event: InjectedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            }]
        );
    }

    #[test]
    fn transport_dispatcher_maps_core_actions_in_order() {
        let transport = LoopbackPeerTransport::default();
        let dispatcher = TransportCoreActionDispatcher::new(transport.clone());
        let crossing = EdgeCrossing {
            peer_id: PeerId::new("right-peer"),
            local_edge: ScreenEdge::Right,
            remote_edge: ScreenEdge::Left,
            exit_position: LogicalPoint { x: 1920, y: 540 },
            edge_offset: 540,
        };

        dispatcher
            .dispatch_core_actions(&[
                CoreAction::StartRemoteSession {
                    peer_id: PeerId::new("right-peer"),
                    crossing: Some(crossing.clone()),
                },
                CoreAction::ForwardInput {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-1")),
                },
            ])
            .expect("transport command dispatch");

        assert_eq!(
            transport.snapshot().expect("loopback command snapshot"),
            vec![
                DaemonTransportCommand::StartRemoteSession {
                    peer_id: PeerId::new("right-peer"),
                    crossing: Some(crossing),
                },
                DaemonTransportCommand::ForwardInput {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                DaemonTransportCommand::ReleaseAllInputs,
                DaemonTransportCommand::StopRemoteSession {
                    session_id: Some(SessionId::new("session-1")),
                },
            ]
        );
    }

    #[test]
    fn peer_transport_message_round_trips_transport_commands() {
        let commands = vec![
            DaemonTransportCommand::StartRemoteSession {
                peer_id: PeerId::new("right-peer"),
                crossing: Some(EdgeCrossing {
                    peer_id: PeerId::new("right-peer"),
                    local_edge: ScreenEdge::Right,
                    remote_edge: ScreenEdge::Left,
                    exit_position: LogicalPoint { x: 1920, y: 540 },
                    edge_offset: 540,
                }),
            },
            DaemonTransportCommand::ForwardInput {
                event: InjectedInputEvent::Key {
                    key: PhysicalKey::Code(44),
                    state: PressState::Pressed,
                },
            },
            DaemonTransportCommand::ForwardInput {
                event: InjectedInputEvent::MouseButton {
                    button: MouseButton::Other(9),
                    state: PressState::Released,
                },
            },
            DaemonTransportCommand::ReleaseAllInputs,
            DaemonTransportCommand::StopRemoteSession {
                session_id: Some(SessionId::new("session-1")),
            },
        ];

        for command in commands {
            let message = PeerTransportMessage::from_command(&command);

            assert_eq!(
                message.into_command().expect("peer command round trip"),
                command
            );
        }
    }

    #[test]
    fn peer_transport_message_rejects_unknown_wire_values() {
        let message = PeerTransportMessage {
            protocol: super::PeerTransportProtocolVersion { major: 1, minor: 0 },
            command: super::PeerTransportCommandPayload::ForwardInput {
                event: super::PeerTransportInputEvent::Key {
                    key: "notARealKey".to_string(),
                    state: "pressed".to_string(),
                },
            },
        };

        let error = message
            .into_command()
            .expect_err("unknown key should be rejected");

        assert_eq!(
            error.to_string(),
            "unsupported peer transport value: notARealKey"
        );
    }

    #[test]
    fn tcp_peer_transport_sends_commands_over_loopback() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_commands(&listener, 4));
        let transport = TcpPeerTransport::new(PeerId::new("right-peer"), address);
        let dispatcher = TransportCoreActionDispatcher::new(transport);
        let crossing = EdgeCrossing {
            peer_id: PeerId::new("right-peer"),
            local_edge: ScreenEdge::Right,
            remote_edge: ScreenEdge::Left,
            exit_position: LogicalPoint { x: 1920, y: 540 },
            edge_offset: 540,
        };

        dispatcher
            .dispatch_core_actions(&[
                CoreAction::StartRemoteSession {
                    peer_id: PeerId::new("right-peer"),
                    crossing: Some(crossing.clone()),
                },
                CoreAction::ForwardInput {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-1")),
                },
            ])
            .expect("TCP peer transport dispatch");
        let commands = server_thread
            .join()
            .expect("TCP peer transport server thread")
            .expect("TCP peer transport commands");

        assert_eq!(
            commands,
            vec![
                DaemonTransportCommand::StartRemoteSession {
                    peer_id: PeerId::new("right-peer"),
                    crossing: Some(crossing),
                },
                DaemonTransportCommand::ForwardInput {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                DaemonTransportCommand::ReleaseAllInputs,
                DaemonTransportCommand::StopRemoteSession {
                    session_id: Some(SessionId::new("session-1")),
                },
            ]
        );
    }

    #[test]
    fn tcp_peer_transport_rejects_start_commands_for_other_peers() {
        let transport = TcpPeerTransport::new(
            PeerId::new("right-peer"),
            "127.0.0.1:9".parse().expect("discard address"),
        );

        let error = transport
            .dispatch_transport_command(DaemonTransportCommand::StartRemoteSession {
                peer_id: PeerId::new("left-peer"),
                crossing: None,
            })
            .expect_err("peer mismatch should be rejected before connect");

        assert_eq!(
            error.to_string(),
            "peer transport configured for right-peer, got start command for left-peer"
        );
    }

    #[test]
    fn transport_dispatcher_returns_transport_failure() {
        let dispatcher = TransportCoreActionDispatcher::new(FailingPeerTransport {
            message: "peer transport unavailable",
        });

        let error = dispatcher
            .dispatch_core_actions(&[CoreAction::StartRemoteSession {
                peer_id: PeerId::new("right-peer"),
                crossing: None,
            }])
            .expect_err("transport error should be returned");

        assert_eq!(error.to_string(), "peer transport unavailable");
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

    #[test]
    fn daemon_ipc_server_reads_shared_runtime_state() {
        let state = shared_runtime_state(RuntimeInputState::new());
        let server =
            DaemonIpcServer::from_shared_state(state.clone(), FakePlatformAdapter::default());
        {
            let mut state = state.lock().expect("runtime state lock");
            state
                .apply_event(akraz_core::RuntimeEvent::RemoteEntryRequested {
                    peer_id: akraz_core::PeerId::new("right-peer"),
                })
                .expect("remote entry request");
        }
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

        assert_eq!(response.result.mode, ControlModeSnapshot::EnteringRemote);
    }

    #[test]
    fn daemon_input_capture_worker_drains_events_into_shared_state() {
        let platform =
            FakePlatformAdapter::default().with_captured_events(vec![CapturedInputEvent::Key {
                key: PhysicalKey::LeftShift,
                state: PressState::Pressed,
            }]);
        let state = shared_runtime_state(RuntimeInputState::new());

        let worker = start_daemon_input_capture(
            state.clone(),
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            let state = state.lock().expect("runtime state lock");
            if state.pressed_keys().contains(&PhysicalKey::LeftShift)
                && state.modifiers().left_shift
            {
                drop(state);
                worker.stop().expect("stop capture worker");
                return;
            }
            drop(state);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to drain preloaded key events");
    }

    #[test]
    fn daemon_input_capture_worker_uses_platform_geometry_for_initial_pointer() {
        let platform = FakePlatformAdapter::default()
            .with_desktop_geometry(desktop_geometry_at_right_edge())
            .with_captured_events(vec![CapturedInputEvent::PointerMoved {
                delta_x: 1,
                delta_y: 0,
            }]);
        let state = shared_runtime_state(RuntimeInputState::new());

        let worker = start_daemon_input_capture(
            state.clone(),
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            let state = state.lock().expect("runtime state lock");
            if state.last_local_pointer() == Some(LogicalPoint { x: 1920, y: 540 }) {
                assert_eq!(state.mode(), ControlMode::Local);
                drop(state);
                worker.stop().expect("stop capture worker");
                return;
            }
            drop(state);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to use platform geometry");
    }

    #[test]
    fn daemon_input_capture_worker_routes_pointer_crossing_into_shared_state() {
        let platform = FakePlatformAdapter::default().with_captured_events(vec![
            CapturedInputEvent::PointerMoved {
                delta_x: 1,
                delta_y: 0,
            },
        ]);
        let state = shared_runtime_state(RuntimeInputState::new());

        let worker = start_daemon_input_capture_with_routing(
            state.clone(),
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
            DaemonInputRoutingConfig {
                screen_layout: right_edge_layout(),
                initial_pointer: LogicalPoint { x: 1919, y: 540 },
            },
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            let state = state.lock().expect("runtime state lock");
            if state.mode() == ControlMode::EnteringRemote {
                assert_eq!(state.pending_peer_id(), Some(&PeerId::new("right-peer")));
                assert_eq!(
                    state.last_local_pointer(),
                    Some(LogicalPoint { x: 1920, y: 540 })
                );
                drop(state);
                worker.stop().expect("stop capture worker");
                return;
            }
            drop(state);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to route pointer crossing");
    }

    #[test]
    fn daemon_input_capture_worker_dispatches_edge_crossing_action() {
        let platform = FakePlatformAdapter::default().with_captured_events(vec![
            CapturedInputEvent::PointerMoved {
                delta_x: 1,
                delta_y: 0,
            },
        ]);
        let state = shared_runtime_state(RuntimeInputState::new());
        let dispatcher = RecordingCoreActionDispatcher::default();
        let expected_action = CoreAction::StartRemoteSession {
            peer_id: PeerId::new("right-peer"),
            crossing: Some(EdgeCrossing {
                peer_id: PeerId::new("right-peer"),
                local_edge: ScreenEdge::Right,
                remote_edge: ScreenEdge::Left,
                exit_position: LogicalPoint { x: 1920, y: 540 },
                edge_offset: 540,
            }),
        };

        let worker = start_daemon_input_capture_with_routing_and_dispatcher(
            state,
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
            DaemonInputRoutingConfig {
                screen_layout: right_edge_layout(),
                initial_pointer: LogicalPoint { x: 1919, y: 540 },
            },
            dispatcher.clone(),
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            if dispatcher.snapshot().contains(&expected_action) {
                worker.stop().expect("stop capture worker");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to dispatch edge crossing action");
    }

    #[test]
    fn daemon_input_capture_worker_dispatches_edge_crossing_transport_command() {
        let platform = FakePlatformAdapter::default().with_captured_events(vec![
            CapturedInputEvent::PointerMoved {
                delta_x: 1,
                delta_y: 0,
            },
        ]);
        let state = shared_runtime_state(RuntimeInputState::new());
        let transport = LoopbackPeerTransport::default();
        let dispatcher = TransportCoreActionDispatcher::new(transport.clone());
        let expected_command = DaemonTransportCommand::StartRemoteSession {
            peer_id: PeerId::new("right-peer"),
            crossing: Some(EdgeCrossing {
                peer_id: PeerId::new("right-peer"),
                local_edge: ScreenEdge::Right,
                remote_edge: ScreenEdge::Left,
                exit_position: LogicalPoint { x: 1920, y: 540 },
                edge_offset: 540,
            }),
        };

        let worker = start_daemon_input_capture_with_routing_and_dispatcher(
            state,
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
            DaemonInputRoutingConfig {
                screen_layout: right_edge_layout(),
                initial_pointer: LogicalPoint { x: 1919, y: 540 },
            },
            dispatcher,
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            if transport
                .snapshot()
                .expect("loopback command snapshot")
                .contains(&expected_command)
            {
                worker.stop().expect("stop capture worker");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to dispatch edge crossing transport command");
    }

    #[test]
    fn daemon_input_capture_worker_dispatches_remote_forward_action() {
        let platform = FakePlatformAdapter::default().with_captured_events(vec![
            CapturedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            },
        ]);
        let mut initial_state = RuntimeInputState::new();
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-1"),
            })
            .expect("remote entry confirmed");
        let state = shared_runtime_state(initial_state);
        let dispatcher = RecordingCoreActionDispatcher::default();
        let expected_action = CoreAction::ForwardInput {
            event: InjectedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            },
        };

        let worker = start_daemon_input_capture_with_routing_and_dispatcher(
            state,
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
            DaemonInputRoutingConfig {
                screen_layout: right_edge_layout(),
                initial_pointer: LogicalPoint { x: 1919, y: 540 },
            },
            dispatcher.clone(),
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            if dispatcher.snapshot().contains(&expected_action) {
                worker.stop().expect("stop capture worker");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to dispatch remote forward action");
    }

    #[test]
    fn daemon_input_capture_worker_routes_configured_edge_binding_from_platform_geometry() {
        let platform = FakePlatformAdapter::default()
            .with_desktop_geometry(desktop_geometry_at_right_edge())
            .with_captured_events(vec![CapturedInputEvent::PointerMoved {
                delta_x: 1,
                delta_y: 0,
            }]);
        let state = shared_runtime_state(RuntimeInputState::new());

        let worker = start_daemon_input_capture_with_edge_bindings(
            state.clone(),
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(1),
            },
            vec![ScreenEdgeBinding {
                local_edge: ScreenEdge::Right,
                peer_id: PeerId::new("right-peer"),
                remote_edge: ScreenEdge::Left,
            }],
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            let state = state.lock().expect("runtime state lock");
            if state.mode() == ControlMode::EnteringRemote {
                assert_eq!(state.pending_peer_id(), Some(&PeerId::new("right-peer")));
                assert_eq!(
                    state.last_local_pointer(),
                    Some(LogicalPoint { x: 1920, y: 540 })
                );
                drop(state);
                worker.stop().expect("stop capture worker");
                return;
            }
            drop(state);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected configured edge binding to route pointer crossing");
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
