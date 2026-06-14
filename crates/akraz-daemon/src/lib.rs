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
    CapturedInputEvent, ControlMode, CoreAction, CoreTransitionError, DeviceId, EdgeCrossing,
    InjectedInputEvent, LogicalPoint, MouseButton, PeerId, PhysicalKey, PressState, RuntimeEvent,
    RuntimeInputState, ScreenEdge, ScreenEdgeBinding, ScreenLayout, SessionId,
};
use akraz_ipc::{
    ControlModeSnapshot, DaemonStatus, InputReleaseAllResult, IpcCodecError,
    IpcPlatformCapabilities, IpcRequest, IpcTransportError, JsonRpcError, JsonRpcFailure,
    JsonRpcSuccess, LocalIpcServer, PermissionIssue, PermissionsProbe, ProtocolVersionSnapshot,
    parse_request_line, serve_os_local_ipc_once, to_json_line,
};
use akraz_platform::{
    DesktopGeometry, InputCaptureConfig, InputCapturePolicy, InputCaptureSession, PlatformAdapter,
    PlatformError,
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

/// Thread-safe shared dispatcher used by IPC and capture workers.
pub type SharedCoreActionDispatcher = Arc<dyn CoreActionDispatcher>;

impl CoreActionDispatcher for SharedCoreActionDispatcher {
    fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError> {
        self.as_ref().dispatch_core_actions(actions)
    }
}

/// No-op dispatcher used until a real peer transport is attached.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopCoreActionDispatcher;

impl CoreActionDispatcher for NoopCoreActionDispatcher {
    fn dispatch_core_actions(&self, _actions: &[CoreAction]) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// Dispatcher decorator that executes local platform recovery actions before delegating the rest.
#[derive(Debug, Clone)]
pub struct LocalPlatformCoreActionDispatcher<P, D> {
    platform: P,
    next: D,
}

impl<P, D> LocalPlatformCoreActionDispatcher<P, D> {
    pub fn new(platform: P, next: D) -> Self {
        Self { platform, next }
    }

    pub fn platform(&self) -> &P {
        &self.platform
    }

    pub fn next(&self) -> &D {
        &self.next
    }
}

impl<P, D> CoreActionDispatcher for LocalPlatformCoreActionDispatcher<P, D>
where
    P: PlatformAdapter + Send + Sync + 'static,
    D: CoreActionDispatcher,
{
    fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError> {
        for action in actions {
            match action {
                CoreAction::ReleaseLocalInputs => self.platform.release_all()?,
                other => self
                    .next
                    .dispatch_core_actions(std::slice::from_ref(other))?,
            }
        }

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

impl DaemonTransportCommand {
    fn from_remote_core_action(action: &CoreAction) -> Option<Self> {
        match action {
            CoreAction::ReleaseLocalInputs => None,
            CoreAction::StartRemoteSession { peer_id, crossing } => {
                Some(Self::StartRemoteSession {
                    peer_id: peer_id.clone(),
                    crossing: crossing.clone(),
                })
            }
            CoreAction::ForwardInput { event } => Some(Self::ForwardInput {
                event: event.clone(),
            }),
            CoreAction::ReleaseAllInputs => Some(Self::ReleaseAllInputs),
            CoreAction::StopRemoteSession { session_id } => Some(Self::StopRemoteSession {
                session_id: session_id.clone(),
            }),
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
            if let Some(command) = DaemonTransportCommand::from_remote_core_action(action) {
                self.transport.dispatch_transport_command(command)?;
            }
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

/// Persistent TCP peer session transport that sends one hello frame, then command frames.
#[derive(Debug, Clone)]
pub struct TcpPeerSessionTransport {
    peer_id: PeerId,
    local_device_id: DeviceId,
    address: SocketAddr,
    stream: Arc<Mutex<TcpStream>>,
}

impl TcpPeerSessionTransport {
    /// Connect to a peer and send the opening session hello frame.
    pub fn connect(
        peer_id: PeerId,
        local_device_id: DeviceId,
        address: SocketAddr,
    ) -> Result<Self, PlatformError> {
        let mut stream = TcpStream::connect(address).map_err(|error| {
            PlatformError::new(format!(
                "failed to connect peer session {} at {}: {error}",
                peer_id.as_str(),
                address
            ))
        })?;
        let hello = PeerTransportSessionFrame::Hello {
            protocol: PeerTransportProtocolVersion::current(),
            device_id: local_device_id.as_str().to_string(),
            peer_id: peer_id.as_str().to_string(),
        };

        write_peer_transport_session_frame(&mut stream, &hello)?;

        Ok(Self {
            peer_id,
            local_device_id,
            address,
            stream: Arc::new(Mutex::new(stream)),
        })
    }

    /// Return the peer this session is connected to.
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Return the local device id announced in the session hello.
    pub fn local_device_id(&self) -> &DeviceId {
        &self.local_device_id
    }

    /// Return the TCP address this session is connected to.
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl DaemonPeerTransport for TcpPeerSessionTransport {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError> {
        validate_transport_command_peer(&self.peer_id, &command)?;

        let frame = PeerTransportSessionFrame::Command {
            command: PeerTransportCommandPayload::from(&command),
        };
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| PlatformError::new("peer session stream is unavailable"))?;

        write_peer_transport_session_frame(&mut stream, &frame)
    }
}

/// Runtime-managed peer session transport used by daemon control commands.
#[derive(Debug, Default, Clone)]
pub struct ManagedPeerSessionTransport {
    active_session: Arc<Mutex<Option<TcpPeerSessionTransport>>>,
}

impl ManagedPeerSessionTransport {
    /// Create an empty managed peer session transport.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return whether a peer session is currently active.
    pub fn is_connected(&self) -> Result<bool, PlatformError> {
        self.active_session
            .lock()
            .map_err(|_| PlatformError::new("managed peer session is unavailable"))
            .map(|session| session.is_some())
    }

    /// Return a snapshot of the active peer session, if one exists.
    pub fn active_session(&self) -> Result<Option<ManagedPeerSessionSnapshot>, PlatformError> {
        self.active_session
            .lock()
            .map_err(|_| PlatformError::new("managed peer session is unavailable"))
            .map(|session| session.as_ref().map(ManagedPeerSessionSnapshot::from))
    }

    /// Attach an already connected TCP peer session.
    pub fn attach_session(&self, session: TcpPeerSessionTransport) -> Result<(), PlatformError> {
        let mut active_session = self
            .active_session
            .lock()
            .map_err(|_| PlatformError::new("managed peer session is unavailable"))?;
        if active_session.is_some() {
            return Err(PlatformError::new(
                "managed peer session already has an active peer",
            ));
        }

        *active_session = Some(session);
        Ok(())
    }

    /// Disconnect the active peer session and return the detached session snapshot.
    pub fn disconnect_session(&self) -> Result<Option<ManagedPeerSessionSnapshot>, PlatformError> {
        let mut active_session = self
            .active_session
            .lock()
            .map_err(|_| PlatformError::new("managed peer session is unavailable"))?;

        Ok(active_session
            .take()
            .as_ref()
            .map(ManagedPeerSessionSnapshot::from))
    }

    fn active_transport(&self) -> Result<Option<TcpPeerSessionTransport>, PlatformError> {
        self.active_session
            .lock()
            .map_err(|_| PlatformError::new("managed peer session is unavailable"))
            .map(|session| session.clone())
    }
}

impl DaemonPeerTransport for ManagedPeerSessionTransport {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError> {
        if let Some(transport) = self.active_transport()? {
            transport.dispatch_transport_command(command)?;
        }

        Ok(())
    }
}

/// Observable active-session facts kept out of the raw TCP stream wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPeerSessionSnapshot {
    pub peer_id: PeerId,
    pub local_device_id: DeviceId,
    pub address: SocketAddr,
}

impl From<&TcpPeerSessionTransport> for ManagedPeerSessionSnapshot {
    fn from(transport: &TcpPeerSessionTransport) -> Self {
        Self {
            peer_id: transport.peer_id().clone(),
            local_device_id: transport.local_device_id().clone(),
            address: transport.address(),
        }
    }
}

/// Decoded persistent TCP peer session captured by a bounded smoke server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTransportSession {
    pub hello: PeerTransportSessionHello,
    pub commands: Vec<DaemonTransportCommand>,
}

/// Peer identity facts announced at the start of a persistent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTransportSessionHello {
    pub protocol: ProtocolVersion,
    pub device_id: DeviceId,
    pub peer_id: PeerId,
}

/// Result of executing one remote peer command against a platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerTransportCommandExecution {
    RemoteSessionStarted {
        peer_id: PeerId,
        crossing: Option<EdgeCrossing>,
    },
    InputForwarded {
        event: InjectedInputEvent,
    },
    InputsReleased,
    RemoteSessionStopped {
        session_id: Option<SessionId>,
    },
}

/// Executed persistent TCP peer session captured by a bounded receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTransportSessionExecution {
    pub hello: PeerTransportSessionHello,
    pub outcomes: Vec<PeerTransportCommandExecution>,
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

/// Serve one persistent TCP peer session and read a bounded command batch.
pub fn serve_tcp_peer_transport_session(
    listener: &TcpListener,
    max_commands: usize,
) -> Result<PeerTransportSession, PlatformError> {
    let (stream, _) = listener
        .accept()
        .map_err(|error| PlatformError::new(format!("failed to accept peer session: {error}")))?;
    let mut reader = BufReader::new(stream);
    let hello = read_peer_transport_session_hello(&mut reader)?;
    let mut commands = Vec::with_capacity(max_commands);

    for _ in 0..max_commands {
        commands.push(read_peer_transport_session_command(&mut reader)?);
    }

    Ok(PeerTransportSession { hello, commands })
}

/// Serve one persistent TCP peer session and execute each command as it arrives.
pub fn serve_tcp_peer_transport_session_and_execute<P>(
    listener: &TcpListener,
    max_commands: usize,
    platform: &P,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    P: PlatformAdapter,
{
    let (stream, _) = listener
        .accept()
        .map_err(|error| PlatformError::new(format!("failed to accept peer session: {error}")))?;
    let mut reader = BufReader::new(stream);
    let hello = read_peer_transport_session_hello(&mut reader)?;
    let mut outcomes = Vec::with_capacity(max_commands);

    for _ in 0..max_commands {
        let command = read_peer_transport_session_command(&mut reader)?;
        outcomes.push(execute_peer_transport_command(platform, &command)?);
    }

    Ok(PeerTransportSessionExecution { hello, outcomes })
}

/// Serve one persistent TCP peer session and execute commands until the peer closes the session.
pub fn serve_tcp_peer_transport_session_and_execute_until_closed<P>(
    listener: &TcpListener,
    platform: &P,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    P: PlatformAdapter,
{
    let (stream, _) = listener
        .accept()
        .map_err(|error| PlatformError::new(format!("failed to accept peer session: {error}")))?;
    let mut reader = BufReader::new(stream);

    execute_peer_transport_session_stream_until_closed(&mut reader, platform)
}

/// Execute a decoded peer transport session from any buffered stream until EOF.
pub fn execute_peer_transport_session_stream_until_closed<R, P>(
    reader: &mut R,
    platform: &P,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    R: BufRead,
    P: PlatformAdapter,
{
    let hello = read_peer_transport_session_hello(reader)?;
    let mut outcomes = Vec::new();

    loop {
        match read_optional_peer_transport_session_frame(reader)? {
            Some(PeerTransportSessionFrame::Command { command }) => {
                let command = command.into_command()?;
                outcomes.push(execute_peer_transport_command(platform, &command)?);
            }
            Some(PeerTransportSessionFrame::Hello { .. }) => {
                return Err(PlatformError::new(
                    "peer session received duplicate hello frame after session start",
                ));
            }
            None => return Ok(PeerTransportSessionExecution { hello, outcomes }),
        }
    }
}

/// Execute one decoded peer transport command against the local platform boundary.
pub fn execute_peer_transport_command<P>(
    platform: &P,
    command: &DaemonTransportCommand,
) -> Result<PeerTransportCommandExecution, PlatformError>
where
    P: PlatformAdapter,
{
    match command {
        DaemonTransportCommand::StartRemoteSession { peer_id, crossing } => {
            Ok(PeerTransportCommandExecution::RemoteSessionStarted {
                peer_id: peer_id.clone(),
                crossing: crossing.clone(),
            })
        }
        DaemonTransportCommand::ForwardInput { event } => {
            platform.inject_input(event)?;
            Ok(PeerTransportCommandExecution::InputForwarded {
                event: event.clone(),
            })
        }
        DaemonTransportCommand::ReleaseAllInputs => {
            platform.release_all()?;
            Ok(PeerTransportCommandExecution::InputsReleased)
        }
        DaemonTransportCommand::StopRemoteSession { session_id } => {
            Ok(PeerTransportCommandExecution::RemoteSessionStopped {
                session_id: session_id.clone(),
            })
        }
    }
}

fn write_peer_transport_session_frame(
    stream: &mut TcpStream,
    frame: &PeerTransportSessionFrame,
) -> Result<(), PlatformError> {
    let line = serde_json::to_string(frame).map_err(|error| {
        PlatformError::new(format!("failed to encode peer session frame: {error}"))
    })?;

    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .and_then(|()| stream.flush())
        .map_err(|error| PlatformError::new(format!("failed to write peer session frame: {error}")))
}

fn read_peer_transport_session_hello<R>(
    reader: &mut R,
) -> Result<PeerTransportSessionHello, PlatformError>
where
    R: BufRead,
{
    match read_peer_transport_session_frame(reader)? {
        PeerTransportSessionFrame::Hello {
            protocol,
            device_id,
            peer_id,
        } => {
            let version = protocol_version_from_wire(protocol)?;
            Ok(PeerTransportSessionHello {
                protocol: version,
                device_id: DeviceId::new(device_id),
                peer_id: PeerId::new(peer_id),
            })
        }
        PeerTransportSessionFrame::Command { .. } => Err(PlatformError::new(
            "peer session expected hello frame before command frames",
        )),
    }
}

fn read_peer_transport_session_command<R>(
    reader: &mut R,
) -> Result<DaemonTransportCommand, PlatformError>
where
    R: BufRead,
{
    match read_peer_transport_session_frame(reader)? {
        PeerTransportSessionFrame::Command { command } => command.into_command(),
        PeerTransportSessionFrame::Hello { .. } => Err(PlatformError::new(
            "peer session received duplicate hello frame after session start",
        )),
    }
}

fn read_peer_transport_session_frame<R>(
    reader: &mut R,
) -> Result<PeerTransportSessionFrame, PlatformError>
where
    R: BufRead,
{
    read_optional_peer_transport_session_frame(reader)?
        .ok_or_else(|| PlatformError::new("peer session closed before the next frame was received"))
}

fn read_optional_peer_transport_session_frame<R>(
    reader: &mut R,
) -> Result<Option<PeerTransportSessionFrame>, PlatformError>
where
    R: BufRead,
{
    let mut line = String::new();
    let read = reader.read_line(&mut line).map_err(|error| {
        PlatformError::new(format!("failed to read peer session frame: {error}"))
    })?;
    if read == 0 {
        return Ok(None);
    }

    serde_json::from_str(&line)
        .map_err(|error| {
            PlatformError::new(format!("failed to decode peer session frame: {error}"))
        })
        .map(Some)
}

fn protocol_version_from_wire(
    protocol: PeerTransportProtocolVersion,
) -> Result<ProtocolVersion, PlatformError> {
    let version = ProtocolVersion {
        major: protocol.major,
        minor: protocol.minor,
    };
    if ProtocolVersion::CURRENT.is_compatible_with(version) {
        Ok(version)
    } else {
        Err(PlatformError::new(format!(
            "unsupported peer transport protocol {}.{}",
            protocol.major, protocol.minor
        )))
    }
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

/// Wire frame used by persistent peer transport sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PeerTransportSessionFrame {
    Hello {
        protocol: PeerTransportProtocolVersion,
        device_id: String,
        peer_id: String,
    },
    Command {
        command: PeerTransportCommandPayload,
    },
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
pub struct DaemonIpcServer<P> {
    state: SharedRuntimeInputState,
    platform: P,
    dispatcher: SharedCoreActionDispatcher,
}

impl<P> DaemonIpcServer<P>
where
    P: PlatformAdapter + Clone + Send + Sync + 'static,
{
    /// Create an in-process daemon IPC server.
    pub fn new(state: RuntimeInputState, platform: P) -> Self {
        Self::from_shared_state(shared_runtime_state(state), platform)
    }

    /// Create an in-process daemon IPC server from shared runtime state.
    pub fn from_shared_state(state: SharedRuntimeInputState, platform: P) -> Self {
        let dispatcher = Arc::new(LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            NoopCoreActionDispatcher,
        ));

        Self::from_shared_state_and_dispatcher(state, platform, dispatcher)
    }
}

impl<P> DaemonIpcServer<P> {
    /// Create an in-process daemon IPC server from shared runtime state and dispatcher.
    pub fn from_shared_state_and_dispatcher(
        state: SharedRuntimeInputState,
        platform: P,
        dispatcher: SharedCoreActionDispatcher,
    ) -> Self {
        Self {
            state,
            platform,
            dispatcher,
        }
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
        let mut state = self.state.lock().map_err(|_| {
            IpcTransportError::request_failed("daemon runtime state is unavailable")
        })?;

        handle_ipc_request_line(&mut state, &self.platform, &self.dispatcher, request_line)
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
    sync_capture_policy_with_state(&capture, &state)?;
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
        sync_capture_policy_with_state(&capture, &state)?;
        match capture.recv_timeout(idle_poll_interval) {
            Ok(event) => {
                dispatch_core_action_batch(
                    &dispatcher,
                    apply_routed_capture_event(&state, &mut router, event)?,
                )?;
                sync_capture_policy_with_state(&capture, &state)?;
                dispatch_core_action_batch(
                    &dispatcher,
                    drain_ready_capture_events(
                        &state,
                        &capture,
                        &mut router,
                        config.bounded_drain_batch_size(),
                    )?,
                )?;
                sync_capture_policy_with_state(&capture, &state)?;
            }
            Err(RecvTimeoutError::Timeout) => {
                sync_capture_policy_with_state(&capture, &state)?;
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }

    capture.stop()
}

fn sync_capture_policy_with_state(
    capture: &InputCaptureSession,
    state: &SharedRuntimeInputState,
) -> Result<(), PlatformError> {
    let mode = state
        .lock()
        .map_err(|_| PlatformError::new("daemon runtime state is unavailable"))?
        .mode();

    capture.set_policy(input_capture_policy_for_control_mode(mode));
    Ok(())
}

fn input_capture_policy_for_control_mode(mode: ControlMode) -> InputCapturePolicy {
    match mode {
        ControlMode::Remote => InputCapturePolicy::ConsumeCapturedInput,
        ControlMode::Local
        | ControlMode::EnteringRemote
        | ControlMode::LeavingRemote
        | ControlMode::Suspended => InputCapturePolicy::PassThrough,
    }
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
    state: &mut RuntimeInputState,
    platform: &impl PlatformAdapter,
    dispatcher: &impl CoreActionDispatcher,
    line: &str,
) -> Result<String, DaemonIpcError> {
    match parse_request_line(line) {
        Ok(request) => handle_ipc_request(state, platform, dispatcher, request),
        Err(failure) => encode_response(&failure),
    }
}

fn handle_ipc_request(
    state: &mut RuntimeInputState,
    platform: &impl PlatformAdapter,
    dispatcher: &impl CoreActionDispatcher,
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
        IpcRequest::InputReleaseAll(request) => {
            match recover_local_control_and_release_inputs(state, dispatcher) {
                Ok(result) => encode_response(&JsonRpcSuccess::new(request.id, result)),
                Err(error) => encode_platform_error(request.id, "input release unavailable", error),
            }
        }
    }
}

/// Recover local control and release inputs through the configured action dispatcher.
pub fn recover_local_control_and_release_inputs(
    state: &mut RuntimeInputState,
    dispatcher: &impl CoreActionDispatcher,
) -> Result<InputReleaseAllResult, PlatformError> {
    let actions = state
        .apply_event(RuntimeEvent::EmergencyRecoveryRequested)
        .map_err(recovery_transition_error)?;
    dispatch_core_action_batch(dispatcher, actions)?;

    Ok(InputReleaseAllResult {
        released: true,
        mode: ControlModeSnapshot::from(state.mode()),
    })
}

fn recovery_transition_error(error: CoreTransitionError) -> PlatformError {
    PlatformError::new(format!("failed to recover local control: {error}"))
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
        CapturedInputEvent, ControlMode, CoreAction, DEFAULT_PANIC_HOTKEY_KEY, DeviceId,
        EdgeCrossing, InjectedInputEvent, LogicalPoint, LogicalRect, LogicalSize, MouseButton,
        PeerId, PhysicalKey, PressState, RuntimeEvent, RuntimeInputState, ScreenEdge,
        ScreenEdgeBinding, ScreenLayout, SessionId,
    };
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, DaemonStatusParams, InputReleaseAllParams,
        InputReleaseAllResult, IpcEndpoint, IpcPlatformCapabilities, JsonRpcFailure,
        JsonRpcRequest, JsonRpcSuccess, LocalIpcServer, METHOD_DAEMON_STATUS,
        METHOD_INPUT_RELEASE_ALL, OsLocalIpcClient, call_json_rpc, to_json_line,
    };
    use akraz_platform::{
        DesktopGeometry, FakePlatformAdapter, InputCaptureConfig, InputCapturePolicy,
        PlatformAdapter, PlatformCapabilities, PlatformError,
    };
    use serde_json::json;

    use super::{
        CapturedInputRouter, CoreActionDispatcher, DAEMON_VERSION, DaemonInputCaptureConfig,
        DaemonInputRoutingConfig, DaemonIpcRunConfig, DaemonIpcServer, DaemonPeerTransport,
        DaemonTransportCommand, LocalPlatformCoreActionDispatcher, LoopbackPeerTransport,
        ManagedPeerSessionSnapshot, ManagedPeerSessionTransport, NoopCoreActionDispatcher,
        PeerTransportCommandExecution, PeerTransportMessage, PeerTransportSessionFrame,
        TcpPeerSessionTransport, TcpPeerTransport, TransportCoreActionDispatcher,
        apply_routed_capture_event_to_state, build_daemon_status, build_permissions_probe,
        drain_capture_events, execute_peer_transport_command, handle_ipc_request_line,
        input_capture_policy_for_control_mode, recover_local_control_and_release_inputs,
        serve_daemon_ipc, serve_tcp_peer_transport_commands, serve_tcp_peer_transport_session,
        serve_tcp_peer_transport_session_and_execute,
        serve_tcp_peer_transport_session_and_execute_until_closed, shared_runtime_state,
        start_daemon_input_capture, start_daemon_input_capture_with_edge_bindings,
        start_daemon_input_capture_with_routing,
        start_daemon_input_capture_with_routing_and_dispatcher, sync_capture_policy_with_state,
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
    fn input_capture_policy_consumes_only_remote_mode() {
        assert_eq!(
            input_capture_policy_for_control_mode(ControlMode::Local),
            InputCapturePolicy::PassThrough
        );
        assert_eq!(
            input_capture_policy_for_control_mode(ControlMode::EnteringRemote),
            InputCapturePolicy::PassThrough
        );
        assert_eq!(
            input_capture_policy_for_control_mode(ControlMode::Remote),
            InputCapturePolicy::ConsumeCapturedInput
        );
        assert_eq!(
            input_capture_policy_for_control_mode(ControlMode::LeavingRemote),
            InputCapturePolicy::PassThrough
        );
        assert_eq!(
            input_capture_policy_for_control_mode(ControlMode::Suspended),
            InputCapturePolicy::PassThrough
        );
    }

    #[test]
    fn capture_policy_sync_follows_shared_remote_state() {
        let platform = FakePlatformAdapter::default();
        let capture = platform
            .start_input_capture(InputCaptureConfig::default())
            .expect("fake capture session");
        let state = shared_runtime_state(RuntimeInputState::new());

        sync_capture_policy_with_state(&capture, &state).expect("sync local policy");

        assert_eq!(
            platform.input_capture_policy(),
            InputCapturePolicy::PassThrough
        );

        {
            let mut state = state.lock().expect("shared state lock");
            state
                .apply_event(RuntimeEvent::RemoteEntryRequested {
                    peer_id: PeerId::new("right-peer"),
                })
                .expect("remote entry request");
            state
                .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                    session_id: SessionId::new("session-1"),
                })
                .expect("remote entry confirmed");
        }

        sync_capture_policy_with_state(&capture, &state).expect("sync remote policy");

        assert_eq!(
            platform.input_capture_policy(),
            InputCapturePolicy::ConsumeCapturedInput
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
    fn transport_dispatcher_ignores_local_release_actions() {
        let transport = LoopbackPeerTransport::default();
        let dispatcher = TransportCoreActionDispatcher::new(transport.clone());

        dispatcher
            .dispatch_core_actions(&[CoreAction::ReleaseLocalInputs, CoreAction::ReleaseAllInputs])
            .expect("transport command dispatch");

        assert_eq!(
            transport.snapshot().expect("loopback command snapshot"),
            vec![DaemonTransportCommand::ReleaseAllInputs]
        );
    }

    #[test]
    fn local_platform_dispatcher_releases_local_inputs_and_delegates_remote_actions() {
        let platform = FakePlatformAdapter::default();
        let transport = LoopbackPeerTransport::default();
        let dispatcher = LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            TransportCoreActionDispatcher::new(transport.clone()),
        );

        dispatcher
            .dispatch_core_actions(&[
                CoreAction::ReleaseLocalInputs,
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-1")),
                },
            ])
            .expect("local platform dispatch");

        assert_eq!(
            platform.release_all_count().expect("local release count"),
            1
        );
        assert_eq!(
            transport.snapshot().expect("loopback command snapshot"),
            vec![
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
    fn tcp_peer_session_sends_hello_then_commands_over_one_connection() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&listener, 4));
        let transport = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");
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
            .expect("TCP peer session dispatch");
        let session = server_thread
            .join()
            .expect("TCP peer session server thread")
            .expect("TCP peer session");

        assert_eq!(
            session.hello.protocol,
            akraz_protocol::ProtocolVersion::CURRENT
        );
        assert_eq!(session.hello.device_id, DeviceId::new("windows-desktop"));
        assert_eq!(session.hello.peer_id, PeerId::new("right-peer"));
        assert_eq!(
            session.commands,
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
    fn managed_peer_session_transport_ignores_dispatch_without_active_session() {
        let transport = ManagedPeerSessionTransport::new();

        transport
            .dispatch_transport_command(DaemonTransportCommand::ReleaseAllInputs)
            .expect("missing active session should be a no-op");

        assert_eq!(transport.is_connected(), Ok(false));
        assert_eq!(transport.active_session(), Ok(None));
    }

    #[test]
    fn managed_peer_session_transport_attaches_dispatches_and_disconnects_session() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&listener, 1));
        let managed = ManagedPeerSessionTransport::new();
        let session = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");

        managed
            .attach_session(session)
            .expect("attach active session");
        assert_eq!(managed.is_connected(), Ok(true));
        assert_eq!(
            managed.active_session(),
            Ok(Some(ManagedPeerSessionSnapshot {
                peer_id: PeerId::new("right-peer"),
                local_device_id: DeviceId::new("windows-desktop"),
                address,
            }))
        );
        managed
            .dispatch_transport_command(DaemonTransportCommand::ReleaseAllInputs)
            .expect("managed dispatch");
        assert_eq!(
            managed.disconnect_session(),
            Ok(Some(ManagedPeerSessionSnapshot {
                peer_id: PeerId::new("right-peer"),
                local_device_id: DeviceId::new("windows-desktop"),
                address,
            }))
        );
        assert_eq!(managed.is_connected(), Ok(false));

        let session = server_thread
            .join()
            .expect("TCP peer session server thread")
            .expect("TCP peer session");
        assert_eq!(session.hello.device_id, DeviceId::new("windows-desktop"));
        assert_eq!(session.hello.peer_id, PeerId::new("right-peer"));
        assert_eq!(
            session.commands,
            vec![DaemonTransportCommand::ReleaseAllInputs]
        );
    }

    #[test]
    fn managed_peer_session_transport_rejects_second_active_session() {
        let first_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind first loopback listener");
        let first_address = first_listener.local_addr().expect("first listener address");
        let first_server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&first_listener, 0));
        let second_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind second loopback listener");
        let second_address = second_listener
            .local_addr()
            .expect("second listener address");
        let second_server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&second_listener, 0));
        let managed = ManagedPeerSessionTransport::new();
        let first_session = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            first_address,
        )
        .expect("connect first TCP peer session");
        let second_session = TcpPeerSessionTransport::connect(
            PeerId::new("left-peer"),
            DeviceId::new("windows-desktop"),
            second_address,
        )
        .expect("connect second TCP peer session");

        managed
            .attach_session(first_session)
            .expect("attach first session");
        let error = managed
            .attach_session(second_session)
            .expect_err("second session should be rejected");

        assert_eq!(
            error.to_string(),
            "managed peer session already has an active peer"
        );
        drop(managed);
        assert!(first_server_thread.join().expect("first server").is_ok());
        assert!(second_server_thread.join().expect("second server").is_ok());
    }

    #[test]
    fn peer_transport_executor_applies_forward_and_release_to_platform() {
        let platform = FakePlatformAdapter::default();

        let forward = execute_peer_transport_command(
            &platform,
            &DaemonTransportCommand::ForwardInput {
                event: InjectedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        )
        .expect("forward input command");
        let release =
            execute_peer_transport_command(&platform, &DaemonTransportCommand::ReleaseAllInputs)
                .expect("release input command");

        assert_eq!(
            forward,
            PeerTransportCommandExecution::InputForwarded {
                event: InjectedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            }
        );
        assert_eq!(release, PeerTransportCommandExecution::InputsReleased);
        assert_eq!(
            platform.injected_events().expect("fake injected events"),
            vec![InjectedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            }]
        );
        assert_eq!(
            platform
                .release_all_count()
                .expect("fake release all count"),
            1
        );
    }

    #[test]
    fn tcp_peer_session_executor_applies_received_commands_to_platform() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread = std::thread::spawn(move || {
            let platform = FakePlatformAdapter::default();
            let execution = serve_tcp_peer_transport_session_and_execute(&listener, 4, &platform)?;
            let injected_events = platform.injected_events()?;
            let release_all_count = platform.release_all_count()?;

            Ok::<_, PlatformError>((execution, injected_events, release_all_count))
        });
        let transport = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");
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
            .expect("TCP peer session dispatch");
        let (execution, injected_events, release_all_count) = server_thread
            .join()
            .expect("TCP peer session executor thread")
            .expect("TCP peer session execution");

        assert_eq!(
            execution.hello.protocol,
            akraz_protocol::ProtocolVersion::CURRENT
        );
        assert_eq!(execution.hello.device_id, DeviceId::new("windows-desktop"));
        assert_eq!(execution.hello.peer_id, PeerId::new("right-peer"));
        assert_eq!(
            execution.outcomes,
            vec![
                PeerTransportCommandExecution::RemoteSessionStarted {
                    peer_id: PeerId::new("right-peer"),
                    crossing: Some(crossing),
                },
                PeerTransportCommandExecution::InputForwarded {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                PeerTransportCommandExecution::InputsReleased,
                PeerTransportCommandExecution::RemoteSessionStopped {
                    session_id: Some(SessionId::new("session-1")),
                },
            ]
        );
        assert_eq!(
            injected_events,
            vec![InjectedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            }]
        );
        assert_eq!(release_all_count, 1);
    }

    #[test]
    fn tcp_peer_session_executor_runs_until_peer_closes() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread = std::thread::spawn(move || {
            let platform = FakePlatformAdapter::default();
            let execution =
                serve_tcp_peer_transport_session_and_execute_until_closed(&listener, &platform)?;
            let injected_events = platform.injected_events()?;
            let release_all_count = platform.release_all_count()?;

            Ok::<_, PlatformError>((execution, injected_events, release_all_count))
        });
        let transport = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");
        let dispatcher = TransportCoreActionDispatcher::new(transport.clone());

        dispatcher
            .dispatch_core_actions(&[
                CoreAction::ForwardInput {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                CoreAction::ReleaseAllInputs,
            ])
            .expect("TCP peer session dispatch");
        drop(dispatcher);
        drop(transport);

        let (execution, injected_events, release_all_count) = server_thread
            .join()
            .expect("TCP peer session executor thread")
            .expect("TCP peer session execution");

        assert_eq!(
            execution.hello.protocol,
            akraz_protocol::ProtocolVersion::CURRENT
        );
        assert_eq!(execution.hello.device_id, DeviceId::new("windows-desktop"));
        assert_eq!(execution.hello.peer_id, PeerId::new("right-peer"));
        assert_eq!(
            execution.outcomes,
            vec![
                PeerTransportCommandExecution::InputForwarded {
                    event: InjectedInputEvent::PointerMoved {
                        delta_x: 8,
                        delta_y: 2,
                    },
                },
                PeerTransportCommandExecution::InputsReleased,
            ]
        );
        assert_eq!(
            injected_events,
            vec![InjectedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            }]
        );
        assert_eq!(release_all_count, 1);
    }

    #[test]
    fn tcp_peer_session_rejects_command_before_hello() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&listener, 1));
        let mut stream = std::net::TcpStream::connect(address).expect("connect loopback server");
        let frame = PeerTransportSessionFrame::Command {
            command: super::PeerTransportCommandPayload::ReleaseAllInputs,
        };
        let line = serde_json::to_string(&frame).expect("session frame JSON");

        std::io::Write::write_all(&mut stream, line.as_bytes()).expect("write command frame");
        std::io::Write::write_all(&mut stream, b"\n").expect("write newline");
        std::io::Write::flush(&mut stream).expect("flush command frame");
        let error = server_thread
            .join()
            .expect("TCP peer session server thread")
            .expect_err("command before hello should be rejected");

        assert_eq!(
            error.to_string(),
            "peer session expected hello frame before command frames"
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
        let mut state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let dispatcher =
            LocalPlatformCoreActionDispatcher::new(platform.clone(), NoopCoreActionDispatcher);
        let request =
            JsonRpcRequest::new("req_1", METHOD_DAEMON_STATUS, DaemonStatusParams::default());
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, &request_line) {
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
    fn release_all_command_recovers_local_control_and_dispatches_release_actions() {
        let mut state = RuntimeInputState::new();
        state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-cli"),
            })
            .expect("remote entry confirmed");
        let dispatcher = RecordingCoreActionDispatcher::default();

        let result = recover_local_control_and_release_inputs(&mut state, &dispatcher)
            .expect("release all command");

        assert_eq!(
            result,
            InputReleaseAllResult {
                released: true,
                mode: ControlModeSnapshot::Local,
            }
        );
        assert_eq!(state.mode(), ControlMode::Local);
        assert_eq!(
            dispatcher.snapshot(),
            vec![
                CoreAction::ReleaseLocalInputs,
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-cli")),
                },
            ]
        );
    }

    #[test]
    fn daemon_ipc_server_handles_input_release_all_request() {
        let mut initial_state = RuntimeInputState::new();
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-ipc"),
            })
            .expect("remote entry confirmed");
        let state = shared_runtime_state(initial_state);
        let platform = FakePlatformAdapter::default();
        let dispatcher = RecordingCoreActionDispatcher::default();
        let server = DaemonIpcServer::from_shared_state_and_dispatcher(
            state.clone(),
            platform,
            std::sync::Arc::new(dispatcher.clone()),
        );
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_INPUT_RELEASE_ALL,
            InputReleaseAllParams::default(),
        );
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line = match server.handle_request_line(&request_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon release-all response: {error}"),
        };
        let response: JsonRpcSuccess<InputReleaseAllResult> =
            match serde_json::from_str(&response_line) {
                Ok(response) => response,
                Err(error) => panic!("expected release-all response JSON: {error}"),
            };

        assert_eq!(response.id, "req_1");
        assert_eq!(
            response.result,
            InputReleaseAllResult {
                released: true,
                mode: ControlModeSnapshot::Local,
            }
        );
        assert_eq!(state.lock().expect("state lock").mode(), ControlMode::Local);
        assert_eq!(
            dispatcher.snapshot(),
            vec![
                CoreAction::ReleaseLocalInputs,
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-ipc")),
                },
            ]
        );
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
    fn daemon_input_capture_worker_dispatches_panic_hotkey_recovery() {
        let platform = FakePlatformAdapter::default().with_captured_events(vec![
            CapturedInputEvent::Key {
                key: PhysicalKey::LeftControl,
                state: PressState::Pressed,
            },
            CapturedInputEvent::Key {
                key: PhysicalKey::LeftAlt,
                state: PressState::Pressed,
            },
            CapturedInputEvent::Key {
                key: DEFAULT_PANIC_HOTKEY_KEY,
                state: PressState::Pressed,
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
        let transport = LoopbackPeerTransport::default();
        let dispatcher = LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            TransportCoreActionDispatcher::new(transport.clone()),
        );
        let expected_release = DaemonTransportCommand::ReleaseAllInputs;
        let expected_stop = DaemonTransportCommand::StopRemoteSession {
            session_id: Some(SessionId::new("session-1")),
        };

        let worker = start_daemon_input_capture_with_routing_and_dispatcher(
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
            dispatcher,
        )
        .expect("daemon capture worker");

        for _ in 0..20 {
            let commands = transport.snapshot().expect("loopback command snapshot");
            if commands.contains(&expected_release)
                && commands.contains(&expected_stop)
                && platform.release_all_count().expect("local release count") == 1
            {
                worker.stop().expect("stop capture worker");
                assert_eq!(
                    state.lock().expect("shared state lock").mode(),
                    ControlMode::Local
                );
                assert_eq!(
                    commands,
                    vec![
                        DaemonTransportCommand::ForwardInput {
                            event: InjectedInputEvent::Key {
                                key: PhysicalKey::LeftControl,
                                state: PressState::Pressed,
                            },
                        },
                        DaemonTransportCommand::ForwardInput {
                            event: InjectedInputEvent::Key {
                                key: PhysicalKey::LeftAlt,
                                state: PressState::Pressed,
                            },
                        },
                        expected_release,
                        expected_stop,
                    ]
                );
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected capture worker to dispatch panic hotkey recovery");
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
        let mut state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let dispatcher =
            LocalPlatformCoreActionDispatcher::new(platform.clone(), NoopCoreActionDispatcher);
        let request_line = r#"{"jsonrpc":"2.0","id":"req_1","method":"daemon.nope","params":{}}"#;

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, request_line) {
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
        let mut state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let dispatcher =
            LocalPlatformCoreActionDispatcher::new(platform.clone(), NoopCoreActionDispatcher);

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, "{not json") {
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
