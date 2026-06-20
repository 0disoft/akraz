//! Daemon status builders shared by akraz daemon and diagnostic clients.

use std::collections::{BTreeSet, VecDeque};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use akraz_core::{
    CapturedInputEvent, ControlMode, CoreAction, CoreTransitionError, DeviceId, EdgeCrossing,
    InjectedInputEvent, LogicalPoint, MouseButton, PeerId, PhysicalKey, PressState, RuntimeEvent,
    RuntimeInputState, ScreenEdge, ScreenEdgeBinding, ScreenLayout, SessionId,
};
use akraz_discovery::{
    DiscoveredPeer, DiscoveryPeerFilter, DiscoverySessionCandidate,
    build_discovery_session_candidates,
};
use akraz_identity::{
    Ed25519PublicKey, FileIdentityStore, IdentityPublicKey, IdentitySecretKey, LocalIdentity,
    TrustedPeer, TrustedPeerIdentity,
};
use akraz_ipc::{
    ControlModeSnapshot, DaemonLogEntry, DaemonLogLevel, DaemonLogsTail, DaemonShutdownResult,
    DaemonStatus, DiagnosticsKeyboardLayout, DiagnosticsScreenTopology, InputReleaseAllResult,
    IpcCodecError, IpcPlatformCapabilities, IpcRequest, IpcTransportError, JsonRpcError,
    JsonRpcFailure, JsonRpcSuccess, LocalIpcServer, PeerStatus, PermissionIssue, PermissionsProbe,
    ProtocolVersionSnapshot, SessionConnectParams, SessionConnectResult, SessionDisconnectResult,
    SessionDiscoveryCandidate, SessionDiscoveryCandidatesResult, SessionStatus, parse_request_line,
    serve_os_local_ipc_once, to_json_line,
};
use akraz_platform::{
    DesktopGeometry, InputCaptureConfig, InputCapturePolicy, InputCaptureSession, PlatformAdapter,
    PlatformCapabilities, PlatformError,
};
use akraz_protocol::{
    AuthProof, AuthTranscript, CapabilityFlags, HANDSHAKE_NONCE_LEN, PeerRole, ProtocolVersion,
    SessionReady, TLS_EXPORTER_LEN,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

/// Current daemon package version.
pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

const JSONRPC_DAEMON_ERROR: i32 = -32000;
const DEFAULT_CAPTURE_DRAIN_BATCH_SIZE: usize = 64;
const DEFAULT_DAEMON_LOG_CAPACITY: usize = 64;
const DEFAULT_DAEMON_LOGS_TAIL_LIMIT: usize = 25;
const DEFAULT_CAPTURE_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(25);
const DEFAULT_CAPTURE_WATCHDOG_IDLE_POLLS: u32 = 80;
const DEFAULT_POWER_RESUME_POLL_GAP: Duration = Duration::from_secs(5);
const DEFAULT_RUNTIME_ENVIRONMENT_POLL_GAP: Duration = Duration::from_millis(250);
const DEFAULT_PEER_SESSION_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_PEER_SESSION_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_PEER_SESSION_RECONNECT_BACKOFF_INITIAL: Duration = Duration::from_millis(250);
const DEFAULT_PEER_SESSION_RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(5);
const PRE_TLS_REMOTE_NONCE: [u8; HANDSHAKE_NONCE_LEN] = [0; HANDSHAKE_NONCE_LEN];
const PRE_TLS_EXPORTER: [u8; TLS_EXPORTER_LEN] = [0; TLS_EXPORTER_LEN];

fn peer_session_capabilities() -> CapabilityFlags {
    CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
}

fn generate_peer_session_nonce() -> [u8; HANDSHAKE_NONCE_LEN] {
    let mut nonce = [0; HANDSHAKE_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

fn build_pre_tls_initiator_auth_transcript(
    local_device_id: impl Into<String>,
    remote_device_id: impl Into<String>,
    local_nonce: [u8; HANDSHAKE_NONCE_LEN],
    local_capabilities: CapabilityFlags,
    remote_capabilities: CapabilityFlags,
) -> AuthTranscript {
    AuthTranscript {
        local_device_id: local_device_id.into(),
        remote_device_id: remote_device_id.into(),
        local_nonce,
        remote_nonce: PRE_TLS_REMOTE_NONCE,
        protocol: ProtocolVersion::CURRENT,
        local_capabilities,
        remote_capabilities,
        role: PeerRole::Initiator,
        tls_exporter: PRE_TLS_EXPORTER,
    }
}

/// Shared daemon runtime state observed by IPC and capture workers.
pub type SharedRuntimeInputState = Arc<Mutex<RuntimeInputState>>;

/// Dispatches side effects requested by the core input state machine.
pub trait CoreActionDispatcher: Send + Sync + 'static {
    fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError>;
}

/// Thread-safe shared dispatcher used by IPC and capture workers.
pub type SharedCoreActionDispatcher = Arc<dyn CoreActionDispatcher>;

/// Shared sanitized daemon event buffer used by diagnostics support bundles.
pub type SharedDaemonLogBuffer = Arc<Mutex<DaemonLogBuffer>>;

impl CoreActionDispatcher for SharedCoreActionDispatcher {
    fn dispatch_core_actions(&self, actions: &[CoreAction]) -> Result<(), PlatformError> {
        self.as_ref().dispatch_core_actions(actions)
    }
}

/// Bounded, sanitized daemon event buffer.
#[derive(Debug, Clone)]
pub struct DaemonLogBuffer {
    entries: VecDeque<DaemonLogEntry>,
    next_sequence: u64,
    capacity: usize,
}

impl DaemonLogBuffer {
    /// Create a bounded daemon log buffer.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            next_sequence: 1,
            capacity,
        }
    }

    /// Record a sanitized daemon event with no user-provided payload values.
    pub fn record(
        &mut self,
        level: DaemonLogLevel,
        event: impl Into<String>,
        message: impl Into<String>,
    ) {
        if self.capacity == 0 {
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(DaemonLogEntry {
            sequence: self.next_sequence,
            level,
            event: event.into(),
            message: message.into(),
        });
        self.next_sequence = self.next_sequence.saturating_add(1);
    }

    /// Return the most recent sanitized daemon events.
    pub fn tail(&self, limit: usize) -> Vec<DaemonLogEntry> {
        let limit = limit.min(self.capacity);
        self.entries
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

/// Build a shared diagnostics log buffer.
pub fn shared_daemon_log_buffer(capacity: usize) -> SharedDaemonLogBuffer {
    Arc::new(Mutex::new(DaemonLogBuffer::new(capacity)))
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
            CoreAction::InputCaptureIdle | CoreAction::ReleaseLocalInputs => None,
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
    stream: Arc<Mutex<TcpPeerSessionStreamState>>,
    _heartbeat_worker: Arc<TcpPeerSessionHeartbeatWorker>,
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
            nonce: None,
            capabilities: peer_session_capabilities(),
        };

        write_peer_transport_session_frame(&mut stream, &hello)?;

        let stream = Arc::new(Mutex::new(TcpPeerSessionStreamState::new(stream)));
        let heartbeat_worker = TcpPeerSessionHeartbeatWorker::start(
            peer_id.clone(),
            address,
            Arc::clone(&stream),
            DEFAULT_PEER_SESSION_HEARTBEAT_INTERVAL,
        );

        Ok(Self {
            peer_id,
            local_device_id,
            address,
            stream,
            _heartbeat_worker: heartbeat_worker,
        })
    }

    /// Connect to a paired peer and send the authenticated session prelude before input frames.
    pub fn connect_authenticated<S>(
        peer_id: PeerId,
        local_identity: &LocalIdentity<S>,
        address: SocketAddr,
        trusted_peer: &TrustedPeer<Ed25519PublicKey>,
    ) -> Result<Self, PlatformError>
    where
        S: IdentitySecretKey,
        S::Error: Display,
    {
        if trusted_peer.identity().peer_id() != peer_id.as_str() {
            return Err(PlatformError::new(format!(
                "trusted peer id {} does not match requested peer {}",
                trusted_peer.identity().peer_id(),
                peer_id.as_str()
            )));
        }

        let local_device_id = DeviceId::new(local_identity.identity().device_id());
        let local_nonce = generate_peer_session_nonce();
        let local_capabilities = peer_session_capabilities();
        let remote_capabilities = trusted_peer.identity().capabilities();
        let transcript = build_pre_tls_initiator_auth_transcript(
            local_identity.identity().device_id(),
            trusted_peer.identity().peer_id(),
            local_nonce,
            local_capabilities,
            remote_capabilities,
        );
        let proof = local_identity
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .map_err(|error| {
                PlatformError::new(format!(
                    "failed to sign peer session auth proof for {}: {error}",
                    peer_id.as_str()
                ))
            })?;

        let mut stream = TcpStream::connect(address).map_err(|error| {
            PlatformError::new(format!(
                "failed to connect peer session {} at {}: {error}",
                peer_id.as_str(),
                address
            ))
        })?;
        let hello = PeerTransportSessionFrame::Hello {
            protocol: PeerTransportProtocolVersion::current(),
            device_id: local_identity.identity().device_id().to_string(),
            peer_id: peer_id.as_str().to_string(),
            nonce: Some(local_nonce),
            capabilities: local_capabilities,
        };
        let ready = PeerTransportSessionFrame::SessionReady {
            ready: SessionReady {
                session_id: format!(
                    "{}->{}",
                    local_identity.identity().device_id(),
                    trusted_peer.identity().peer_id()
                ),
                sequence_base: 0,
                capabilities: local_capabilities.intersection(remote_capabilities),
            },
        };

        write_peer_transport_session_frame(&mut stream, &hello)?;
        write_peer_transport_session_frame(
            &mut stream,
            &PeerTransportSessionFrame::AuthProof { proof },
        )?;
        write_peer_transport_session_frame(&mut stream, &ready)?;

        let stream = Arc::new(Mutex::new(TcpPeerSessionStreamState::new(stream)));
        let heartbeat_worker = TcpPeerSessionHeartbeatWorker::start(
            peer_id.clone(),
            address,
            Arc::clone(&stream),
            DEFAULT_PEER_SESSION_HEARTBEAT_INTERVAL,
        );

        Ok(Self {
            peer_id,
            local_device_id,
            address,
            stream,
            _heartbeat_worker: heartbeat_worker,
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

#[derive(Debug)]
struct TcpPeerSessionStreamState {
    stream: TcpStream,
    next_sequence: u64,
}

impl TcpPeerSessionStreamState {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            next_sequence: 0,
        }
    }

    fn next_sequence(&mut self) -> Result<u64, PlatformError> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.checked_add(1).ok_or_else(|| {
            PlatformError::new("peer session sequence number overflowed before frame write")
        })?;
        Ok(sequence)
    }
}

#[derive(Debug)]
struct TcpPeerSessionHeartbeatWorker {
    running: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl TcpPeerSessionHeartbeatWorker {
    fn start(
        peer_id: PeerId,
        address: SocketAddr,
        stream: Arc<Mutex<TcpPeerSessionStreamState>>,
        interval: Duration,
    ) -> Arc<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let handle = thread::spawn(move || {
            while worker_running.load(Ordering::Acquire) {
                thread::park_timeout(interval);
                if !worker_running.load(Ordering::Acquire) {
                    break;
                }

                let result = stream
                    .lock()
                    .map_err(|_| PlatformError::new("peer session stream is unavailable"))
                    .and_then(|mut stream| {
                        write_peer_transport_session_heartbeat_frame(&mut stream)
                    });
                if let Err(error) = result {
                    eprintln!(
                        "peer session heartbeat stopped for {} at {}: {error}",
                        peer_id.as_str(),
                        address
                    );
                    break;
                }
            }
        });

        Arc::new(Self {
            running,
            handle: Mutex::new(Some(handle)),
        })
    }
}

impl Drop for TcpPeerSessionHeartbeatWorker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        let Some(handle) = self.handle.lock().ok().and_then(|mut handle| handle.take()) else {
            return;
        };
        handle.thread().unpark();
        if handle.join().is_err() {
            eprintln!("peer session heartbeat thread panicked");
        }
    }
}

impl DaemonPeerTransport for TcpPeerSessionTransport {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError> {
        validate_transport_command_peer(&self.peer_id, &command)?;

        let command = PeerTransportCommandPayload::from(&command);
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| PlatformError::new("peer session stream is unavailable"))?;

        write_peer_transport_session_command_frame(&mut stream, command)
    }
}

#[derive(Debug, Clone)]
struct PeerSessionReconnectBackoff {
    consecutive_failures: u32,
    next_allowed_at: Option<Instant>,
    initial_delay: Duration,
    max_delay: Duration,
}

impl Default for PeerSessionReconnectBackoff {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            next_allowed_at: None,
            initial_delay: DEFAULT_PEER_SESSION_RECONNECT_BACKOFF_INITIAL,
            max_delay: DEFAULT_PEER_SESSION_RECONNECT_BACKOFF_MAX,
        }
    }
}

impl PeerSessionReconnectBackoff {
    fn retry_after(&self, now: Instant) -> Option<Duration> {
        self.next_allowed_at
            .and_then(|deadline| deadline.checked_duration_since(now))
            .filter(|remaining| !remaining.is_zero())
    }

    fn record_failure(&mut self, now: Instant) -> Duration {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = self.delay_for_failure(self.consecutive_failures);
        self.next_allowed_at = Some(now.checked_add(delay).unwrap_or(now));
        delay
    }

    fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.next_allowed_at = None;
    }

    fn delay_for_failure(&self, failure_count: u32) -> Duration {
        if failure_count == 0 {
            return Duration::ZERO;
        }

        let mut delay = self.initial_delay.min(self.max_delay);
        for _ in 1..failure_count {
            delay = delay.checked_mul(2).unwrap_or(self.max_delay);
            if delay >= self.max_delay {
                return self.max_delay;
            }
        }

        delay
    }
}

/// Runtime-managed peer session transport used by daemon control commands.
#[derive(Debug, Default, Clone)]
pub struct ManagedPeerSessionTransport {
    active_session: Arc<Mutex<Option<TcpPeerSessionTransport>>>,
    reconnect_backoff: Arc<Mutex<PeerSessionReconnectBackoff>>,
    auth_config: Option<ManagedPeerSessionAuthConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPeerSessionAuthConfig {
    identity_store: PathBuf,
    identity_display_name: String,
}

impl ManagedPeerSessionTransport {
    /// Create an empty managed peer session transport.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty managed peer session transport backed by paired identity storage.
    pub fn with_identity_store(
        identity_store: impl Into<PathBuf>,
        identity_display_name: impl Into<String>,
    ) -> Self {
        Self {
            active_session: Arc::new(Mutex::new(None)),
            reconnect_backoff: Arc::new(Mutex::new(PeerSessionReconnectBackoff::default())),
            auth_config: Some(ManagedPeerSessionAuthConfig {
                identity_store: identity_store.into(),
                identity_display_name: identity_display_name.into(),
            }),
        }
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

    /// Return the trusted display name for a paired peer, when the manager has pairing storage.
    fn trusted_peer_display_name(&self, peer_id: &PeerId) -> Result<Option<String>, PlatformError> {
        let Some(auth_config) = &self.auth_config else {
            return Ok(None);
        };
        let store = FileIdentityStore::new(&auth_config.identity_store);
        let trusted_peer = store.load_trusted_peer(peer_id.as_str()).map_err(|error| {
            PlatformError::new(format!(
                "failed to load trusted peer {} from {}: {error}",
                peer_id.as_str(),
                auth_config.identity_store.display()
            ))
        })?;

        Ok(Some(trusted_peer.identity().display_name().to_string()))
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

    /// Connect and attach a peer session using the manager's configured security boundary.
    pub fn connect_session(
        &self,
        peer_id: PeerId,
        local_device_id: DeviceId,
        address: SocketAddr,
    ) -> Result<(), PlatformError> {
        if self.is_connected()? {
            return Err(PlatformError::new(
                "managed peer session already has an active peer",
            ));
        }

        self.ensure_connect_backoff_allows(Instant::now())?;

        let Some(auth_config) = &self.auth_config else {
            return Err(PlatformError::new(
                "managed peer session requires identity store before session.connect",
            ));
        };
        let store = FileIdentityStore::new(&auth_config.identity_store);
        let local = store
            .load_or_create(&auth_config.identity_display_name)
            .map_err(|error| {
                PlatformError::new(format!(
                    "failed to load identity store {}: {error}",
                    auth_config.identity_store.display()
                ))
            })?
            .into_local_identity();
        if local.identity().device_id() != local_device_id.as_str() {
            return Err(PlatformError::new(format!(
                "session local device id {} does not match identity store device id {}",
                local_device_id.as_str(),
                local.identity().device_id()
            )));
        }
        let trusted_peer = store.load_trusted_peer(peer_id.as_str()).map_err(|error| {
            PlatformError::new(format!(
                "failed to load trusted peer {} from {}: {error}",
                peer_id.as_str(),
                auth_config.identity_store.display()
            ))
        })?;
        let session = match TcpPeerSessionTransport::connect_authenticated(
            peer_id,
            &local,
            address,
            &trusted_peer,
        ) {
            Ok(session) => session,
            Err(error) => return Err(self.record_session_failure(Instant::now(), error)),
        };

        self.attach_session(session)?;
        self.reset_connect_backoff()
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

    fn ensure_connect_backoff_allows(&self, now: Instant) -> Result<(), PlatformError> {
        let backoff = self.reconnect_backoff.lock().map_err(|_| {
            PlatformError::new("managed peer session reconnect backoff is unavailable")
        })?;
        if let Some(retry_after) = backoff.retry_after(now) {
            return Err(PlatformError::new(format!(
                "peer session reconnect backoff active for {}ms",
                retry_after.as_millis()
            )));
        }

        Ok(())
    }

    fn record_session_failure(&self, now: Instant, error: PlatformError) -> PlatformError {
        let Ok(mut backoff) = self.reconnect_backoff.lock() else {
            return PlatformError::new(format!(
                "{error}; managed peer session reconnect backoff is unavailable"
            ));
        };
        let delay = backoff.record_failure(now);

        PlatformError::new(format!(
            "{error}; retry peer session connect after {}ms",
            delay.as_millis()
        ))
    }

    fn reset_connect_backoff(&self) -> Result<(), PlatformError> {
        self.reconnect_backoff
            .lock()
            .map_err(|_| {
                PlatformError::new("managed peer session reconnect backoff is unavailable")
            })
            .map(|mut backoff| backoff.reset())
    }
}

impl DaemonPeerTransport for ManagedPeerSessionTransport {
    fn dispatch_transport_command(
        &self,
        command: DaemonTransportCommand,
    ) -> Result<(), PlatformError> {
        let should_disconnect = matches!(command, DaemonTransportCommand::StopRemoteSession { .. });
        let Some(transport) = self.active_transport()? else {
            return Ok(());
        };
        let dispatch_result = transport.dispatch_transport_command(command);
        let dispatch_result =
            dispatch_result.map_err(|error| self.record_session_failure(Instant::now(), error));
        if should_disconnect || dispatch_result.is_err() {
            match self.disconnect_session() {
                Ok(_) => {}
                Err(disconnect_error) => {
                    return match dispatch_result {
                        Ok(()) => Err(disconnect_error),
                        Err(dispatch_error) => Err(PlatformError::new(format!(
                            "{dispatch_error}; additionally failed to detach managed peer session: {disconnect_error}"
                        ))),
                    };
                }
            }
        }

        dispatch_result
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
    pub nonce: Option<[u8; HANDSHAKE_NONCE_LEN]>,
    pub capabilities: CapabilityFlags,
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
    let mut sequence = PeerTransportSessionSequence::new();

    for _ in 0..max_commands {
        commands.push(read_peer_transport_session_command_with_sequence(
            &mut reader,
            &mut sequence,
        )?);
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
    let mut sequence = PeerTransportSessionSequence::new();

    for _ in 0..max_commands {
        let command =
            read_peer_transport_session_command_with_sequence(&mut reader, &mut sequence)?;
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
    serve_tcp_peer_transport_session_and_execute_until_closed_with_timeout(
        listener,
        platform,
        DEFAULT_PEER_SESSION_HEARTBEAT_TIMEOUT,
    )
}

/// Serve one persistent TCP peer session and execute commands until EOF or heartbeat timeout.
pub fn serve_tcp_peer_transport_session_and_execute_until_closed_with_timeout<P>(
    listener: &TcpListener,
    platform: &P,
    heartbeat_timeout: Duration,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    P: PlatformAdapter,
{
    let (stream, _) = listener
        .accept()
        .map_err(|error| PlatformError::new(format!("failed to accept peer session: {error}")))?;

    execute_tcp_peer_transport_session_until_closed_with_timeout(
        stream,
        platform,
        heartbeat_timeout,
    )
}

/// Execute a TCP peer transport session until EOF or heartbeat timeout.
pub fn execute_tcp_peer_transport_session_until_closed<P>(
    stream: TcpStream,
    platform: &P,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    P: PlatformAdapter,
{
    execute_tcp_peer_transport_session_until_closed_with_timeout(
        stream,
        platform,
        DEFAULT_PEER_SESSION_HEARTBEAT_TIMEOUT,
    )
}

/// Execute a TCP peer transport session with an explicit heartbeat timeout.
pub fn execute_tcp_peer_transport_session_until_closed_with_timeout<P>(
    stream: TcpStream,
    platform: &P,
    heartbeat_timeout: Duration,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    P: PlatformAdapter,
{
    stream
        .set_read_timeout(Some(heartbeat_timeout))
        .map_err(|error| {
            PlatformError::new(format!(
                "failed to configure peer session heartbeat timeout: {error}"
            ))
        })?;
    let mut reader = BufReader::new(stream);

    execute_peer_transport_session_stream_until_closed(&mut reader, platform)
}

/// Execute an authenticated TCP peer transport session with an explicit heartbeat timeout.
pub fn execute_paired_tcp_peer_transport_session_until_closed_with_timeout<P, F>(
    stream: TcpStream,
    platform: &P,
    heartbeat_timeout: Duration,
    local_device_id: &DeviceId,
    trusted_peer_lookup: F,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    P: PlatformAdapter,
    F: FnOnce(&DeviceId) -> Result<TrustedPeer<Ed25519PublicKey>, PlatformError>,
{
    stream
        .set_read_timeout(Some(heartbeat_timeout))
        .map_err(|error| {
            PlatformError::new(format!(
                "failed to configure peer session heartbeat timeout: {error}"
            ))
        })?;
    let mut reader = BufReader::new(stream);

    execute_paired_peer_transport_session_stream_until_closed(
        &mut reader,
        platform,
        local_device_id,
        trusted_peer_lookup,
    )
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
    execute_peer_transport_session_frames_until_closed(reader, platform, hello)
}

/// Execute a decoded authenticated peer transport session after proving the trusted peer identity.
pub fn execute_authenticated_peer_transport_session_stream_until_closed<R, P, V>(
    reader: &mut R,
    platform: &P,
    trusted_peer: &TrustedPeer<V>,
    expected_role: PeerRole,
    transcript: &AuthTranscript,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    R: BufRead,
    P: PlatformAdapter,
    V: IdentityPublicKey,
    V::Error: Display,
{
    let hello = read_peer_transport_session_hello(reader)?;
    let proof = read_peer_transport_session_auth_proof(reader)?;
    trusted_peer
        .verify_auth_proof(expected_role, transcript, &proof)
        .map_err(|error| {
            PlatformError::new(format!(
                "peer session auth proof rejected for {} ({}): {error}",
                trusted_peer.identity().peer_id(),
                trusted_peer.identity().fingerprint()
            ))
        })?;
    let ready = read_peer_transport_session_ready(reader)?;
    if ready.sequence_base != 0 {
        return Err(PlatformError::new(format!(
            "peer session sequence base {} is not supported yet",
            ready.sequence_base
        )));
    }

    execute_peer_transport_session_frames_until_closed(reader, platform, hello)
}

/// Execute a paired peer session by resolving the trusted sender from the opening hello frame.
pub fn execute_paired_peer_transport_session_stream_until_closed<R, P, F>(
    reader: &mut R,
    platform: &P,
    local_device_id: &DeviceId,
    trusted_peer_lookup: F,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    R: BufRead,
    P: PlatformAdapter,
    F: FnOnce(&DeviceId) -> Result<TrustedPeer<Ed25519PublicKey>, PlatformError>,
{
    let hello = read_peer_transport_session_hello(reader)?;
    let trusted_peer = trusted_peer_lookup(&hello.device_id)?;
    let nonce = hello
        .nonce
        .ok_or_else(|| PlatformError::new("peer session hello is missing authentication nonce"))?;
    let transcript = build_pre_tls_initiator_auth_transcript(
        hello.device_id.as_str(),
        local_device_id.as_str(),
        nonce,
        hello.capabilities,
        peer_session_capabilities(),
    );
    let proof = read_peer_transport_session_auth_proof(reader)?;
    trusted_peer
        .verify_auth_proof(PeerRole::Initiator, &transcript, &proof)
        .map_err(|error| {
            PlatformError::new(format!(
                "peer session auth proof rejected for {} ({}): {error}",
                trusted_peer.identity().peer_id(),
                trusted_peer.identity().fingerprint()
            ))
        })?;
    let ready = read_peer_transport_session_ready(reader)?;
    if ready.sequence_base != 0 {
        return Err(PlatformError::new(format!(
            "peer session sequence base {} is not supported yet",
            ready.sequence_base
        )));
    }

    execute_peer_transport_session_frames_until_closed(reader, platform, hello)
}

fn execute_peer_transport_session_frames_until_closed<R, P>(
    reader: &mut R,
    platform: &P,
    hello: PeerTransportSessionHello,
) -> Result<PeerTransportSessionExecution, PlatformError>
where
    R: BufRead,
    P: PlatformAdapter,
{
    let mut outcomes = Vec::new();
    let mut needs_release_on_close = false;
    let mut sequence = PeerTransportSessionSequence::new();

    loop {
        let frame = match read_optional_peer_transport_session_frame(reader) {
            Ok(frame) => frame,
            Err(error) => {
                return Err(release_peer_transport_inputs_after_error(
                    platform,
                    needs_release_on_close,
                    error,
                ));
            }
        };

        match frame {
            Some(PeerTransportSessionFrame::Command {
                sequence: received_sequence,
                command,
            }) => {
                if let Err(error) = sequence.accept(received_sequence) {
                    return Err(release_peer_transport_inputs_after_error(
                        platform,
                        needs_release_on_close,
                        error,
                    ));
                }
                let command = match command.into_command() {
                    Ok(command) => command,
                    Err(error) => {
                        return Err(release_peer_transport_inputs_after_error(
                            platform,
                            needs_release_on_close,
                            error,
                        ));
                    }
                };
                let outcome = match execute_peer_transport_command(platform, &command) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Err(release_peer_transport_inputs_after_error(
                            platform,
                            needs_release_on_close,
                            error,
                        ));
                    }
                };
                update_peer_transport_release_guard(&mut needs_release_on_close, &command);
                outcomes.push(outcome);
            }
            Some(PeerTransportSessionFrame::Hello { .. }) => {
                return Err(release_peer_transport_inputs_after_error(
                    platform,
                    needs_release_on_close,
                    PlatformError::new(
                        "peer session received duplicate hello frame after session start",
                    ),
                ));
            }
            Some(PeerTransportSessionFrame::AuthProof { .. }) => {
                return Err(release_peer_transport_inputs_after_error(
                    platform,
                    needs_release_on_close,
                    PlatformError::new(
                        "peer session received duplicate auth proof frame after session start",
                    ),
                ));
            }
            Some(PeerTransportSessionFrame::SessionReady { .. }) => {
                return Err(release_peer_transport_inputs_after_error(
                    platform,
                    needs_release_on_close,
                    PlatformError::new(
                        "peer session received duplicate session ready frame after session start",
                    ),
                ));
            }
            Some(PeerTransportSessionFrame::Heartbeat {
                sequence: received_sequence,
            }) => {
                if let Err(error) = sequence.accept(received_sequence) {
                    return Err(release_peer_transport_inputs_after_error(
                        platform,
                        needs_release_on_close,
                        error,
                    ));
                }
            }
            None => {
                if needs_release_on_close {
                    platform.release_all()?;
                    outcomes.push(PeerTransportCommandExecution::InputsReleased);
                }

                return Ok(PeerTransportSessionExecution { hello, outcomes });
            }
        }
    }
}

fn update_peer_transport_release_guard(
    needs_release_on_close: &mut bool,
    command: &DaemonTransportCommand,
) {
    match command {
        DaemonTransportCommand::StartRemoteSession { .. }
        | DaemonTransportCommand::ForwardInput { .. } => {
            *needs_release_on_close = true;
        }
        DaemonTransportCommand::ReleaseAllInputs => {
            *needs_release_on_close = false;
        }
        DaemonTransportCommand::StopRemoteSession { .. } => {}
    }
}

fn release_peer_transport_inputs_after_error<P>(
    platform: &P,
    needs_release_on_close: bool,
    error: PlatformError,
) -> PlatformError
where
    P: PlatformAdapter,
{
    if !needs_release_on_close {
        return error;
    }

    match platform.release_all() {
        Ok(()) => error,
        Err(release_error) => PlatformError::new(format!(
            "{error}; additionally failed to release peer session inputs: {release_error}"
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeerTransportSessionSequence {
    expected: u64,
}

impl PeerTransportSessionSequence {
    fn new() -> Self {
        Self { expected: 0 }
    }

    fn accept(&mut self, received: u64) -> Result<(), PlatformError> {
        if received != self.expected {
            return Err(PlatformError::new(format!(
                "peer session sequence mismatch: expected {}, received {}",
                self.expected, received
            )));
        }

        self.expected = self.expected.checked_add(1).ok_or_else(|| {
            PlatformError::new("peer session sequence number overflowed after frame read")
        })?;
        Ok(())
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

fn write_peer_transport_session_command_frame(
    stream: &mut TcpPeerSessionStreamState,
    command: PeerTransportCommandPayload,
) -> Result<(), PlatformError> {
    let sequence = stream.next_sequence()?;
    let frame = PeerTransportSessionFrame::Command { sequence, command };
    write_peer_transport_session_frame(&mut stream.stream, &frame)
}

fn write_peer_transport_session_heartbeat_frame(
    stream: &mut TcpPeerSessionStreamState,
) -> Result<(), PlatformError> {
    let sequence = stream.next_sequence()?;
    let frame = PeerTransportSessionFrame::Heartbeat { sequence };
    write_peer_transport_session_frame(&mut stream.stream, &frame)
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
            nonce,
            capabilities,
        } => {
            let version = protocol_version_from_wire(protocol)?;
            Ok(PeerTransportSessionHello {
                protocol: version,
                device_id: DeviceId::new(device_id),
                peer_id: PeerId::new(peer_id),
                nonce,
                capabilities,
            })
        }
        PeerTransportSessionFrame::Command { .. } => Err(PlatformError::new(
            "peer session expected hello frame before command frames",
        )),
        PeerTransportSessionFrame::AuthProof { .. } => Err(PlatformError::new(
            "peer session expected hello frame before auth proof frames",
        )),
        PeerTransportSessionFrame::SessionReady { .. } => Err(PlatformError::new(
            "peer session expected hello frame before session ready frames",
        )),
        PeerTransportSessionFrame::Heartbeat { .. } => Err(PlatformError::new(
            "peer session expected hello frame before heartbeat frames",
        )),
    }
}

fn read_peer_transport_session_auth_proof<R>(reader: &mut R) -> Result<AuthProof, PlatformError>
where
    R: BufRead,
{
    match read_peer_transport_session_frame(reader)? {
        PeerTransportSessionFrame::AuthProof { proof } => Ok(proof),
        PeerTransportSessionFrame::Hello { .. } => Err(PlatformError::new(
            "peer session received duplicate hello frame before auth proof",
        )),
        PeerTransportSessionFrame::SessionReady { .. } => Err(PlatformError::new(
            "peer session expected auth proof frame before session ready frames",
        )),
        PeerTransportSessionFrame::Command { .. } => Err(PlatformError::new(
            "peer session expected auth proof frame before command frames",
        )),
        PeerTransportSessionFrame::Heartbeat { .. } => Err(PlatformError::new(
            "peer session expected auth proof frame before heartbeat frames",
        )),
    }
}

fn read_peer_transport_session_ready<R>(reader: &mut R) -> Result<SessionReady, PlatformError>
where
    R: BufRead,
{
    match read_peer_transport_session_frame(reader)? {
        PeerTransportSessionFrame::SessionReady { ready } => Ok(ready),
        PeerTransportSessionFrame::Hello { .. } => Err(PlatformError::new(
            "peer session received duplicate hello frame before session ready",
        )),
        PeerTransportSessionFrame::AuthProof { .. } => Err(PlatformError::new(
            "peer session received duplicate auth proof frame before session ready",
        )),
        PeerTransportSessionFrame::Command { .. } => Err(PlatformError::new(
            "peer session expected session ready frame before command frames",
        )),
        PeerTransportSessionFrame::Heartbeat { .. } => Err(PlatformError::new(
            "peer session expected session ready frame before heartbeat frames",
        )),
    }
}

fn read_peer_transport_session_command_with_sequence<R>(
    reader: &mut R,
    sequence: &mut PeerTransportSessionSequence,
) -> Result<DaemonTransportCommand, PlatformError>
where
    R: BufRead,
{
    loop {
        match read_peer_transport_session_frame(reader)? {
            PeerTransportSessionFrame::Command {
                sequence: received_sequence,
                command,
            } => {
                sequence.accept(received_sequence)?;
                return command.into_command();
            }
            PeerTransportSessionFrame::Hello { .. } => {
                return Err(PlatformError::new(
                    "peer session received duplicate hello frame after session start",
                ));
            }
            PeerTransportSessionFrame::AuthProof { .. } => {
                return Err(PlatformError::new(
                    "peer session received auth proof frame after session start",
                ));
            }
            PeerTransportSessionFrame::SessionReady { .. } => {
                return Err(PlatformError::new(
                    "peer session received session ready frame after session start",
                ));
            }
            PeerTransportSessionFrame::Heartbeat {
                sequence: received_sequence,
            } => {
                sequence.accept(received_sequence)?;
            }
        }
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
    ProtocolVersion::negotiate_with_current(version).map_err(|_| {
        PlatformError::new(format!(
            "unsupported peer transport protocol {}.{}",
            protocol.major, protocol.minor
        ))
    })
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
        if ProtocolVersion::negotiate_with_current(version).is_err() {
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
        #[serde(default)]
        nonce: Option<[u8; HANDSHAKE_NONCE_LEN]>,
        #[serde(default)]
        capabilities: CapabilityFlags,
    },
    AuthProof {
        proof: AuthProof,
    },
    SessionReady {
        ready: SessionReady,
    },
    Command {
        sequence: u64,
        command: PeerTransportCommandPayload,
    },
    Heartbeat {
        sequence: u64,
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
    peer_sessions: ManagedPeerSessionTransport,
    logs: SharedDaemonLogBuffer,
    shutdown_requested: Arc<AtomicBool>,
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
        Self::from_shared_state_dispatcher_and_peer_sessions(
            state,
            platform,
            dispatcher,
            ManagedPeerSessionTransport::new(),
        )
    }

    /// Create an in-process daemon IPC server from shared runtime state, dispatcher, and sessions.
    pub fn from_shared_state_dispatcher_and_peer_sessions(
        state: SharedRuntimeInputState,
        platform: P,
        dispatcher: SharedCoreActionDispatcher,
        peer_sessions: ManagedPeerSessionTransport,
    ) -> Self {
        Self::from_shared_state_dispatcher_peer_sessions_and_logs(
            state,
            platform,
            dispatcher,
            peer_sessions,
            shared_daemon_log_buffer(DEFAULT_DAEMON_LOG_CAPACITY),
        )
    }

    /// Create an in-process daemon IPC server with an explicit diagnostics log buffer.
    pub fn from_shared_state_dispatcher_peer_sessions_and_logs(
        state: SharedRuntimeInputState,
        platform: P,
        dispatcher: SharedCoreActionDispatcher,
        peer_sessions: ManagedPeerSessionTransport,
        logs: SharedDaemonLogBuffer,
    ) -> Self {
        Self {
            state,
            platform,
            dispatcher,
            peer_sessions,
            logs,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Return the runtime state shared by daemon background workers.
    pub fn shared_state(&self) -> SharedRuntimeInputState {
        Arc::clone(&self.state)
    }

    /// Return the diagnostics log buffer shared by daemon background workers.
    pub fn shared_logs(&self) -> SharedDaemonLogBuffer {
        Arc::clone(&self.logs)
    }

    /// Return whether a graceful shutdown was requested through local IPC.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::Acquire)
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

        handle_ipc_request_line_with_peer_sessions(
            &mut state,
            &self.platform,
            &self.dispatcher,
            &self.peer_sessions,
            request_line,
            Some(&self.logs),
            Some(&self.shutdown_requested),
        )
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
        if server.shutdown_requested() {
            return Ok(());
        }
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

    fn bounded_idle_watchdog_timeout(self) -> Duration {
        self.bounded_idle_poll_interval()
            .saturating_mul(DEFAULT_CAPTURE_WATCHDOG_IDLE_POLLS)
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

/// Start daemon input capture with configured edges, dispatcher, and diagnostics logs.
pub fn start_daemon_input_capture_with_edge_bindings_dispatcher_and_logs<D>(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    edge_bindings: Vec<ScreenEdgeBinding>,
    dispatcher: D,
    logs: SharedDaemonLogBuffer,
) -> Result<DaemonInputCaptureWorker, PlatformError>
where
    D: CoreActionDispatcher,
{
    let geometry = platform.read_desktop_geometry()?;

    start_daemon_input_capture_with_routing_dispatcher_and_logs(
        state,
        platform,
        config,
        DaemonInputRoutingConfig::from_desktop_geometry(geometry, edge_bindings),
        dispatcher,
        Some(logs),
    )
}

/// Start daemon input capture with runtime environment monitoring and diagnostics logs.
pub fn start_monitored_daemon_input_capture_with_edge_bindings_dispatcher_and_logs<P, D>(
    state: SharedRuntimeInputState,
    platform: &P,
    config: DaemonInputCaptureConfig,
    edge_bindings: Vec<ScreenEdgeBinding>,
    dispatcher: D,
    logs: SharedDaemonLogBuffer,
) -> Result<DaemonInputCaptureWorker, PlatformError>
where
    P: PlatformAdapter + Clone + Send + 'static,
    D: CoreActionDispatcher,
{
    let geometry = platform.read_desktop_geometry()?;

    start_daemon_input_capture_with_routing_dispatcher_logs_and_environment_inspector(
        state,
        platform,
        config,
        DaemonInputRoutingConfig::from_desktop_geometry(geometry, edge_bindings),
        dispatcher,
        Some(logs),
        PlatformRuntimeEnvironmentInspector::new(platform.clone(), Instant::now()),
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
    start_daemon_input_capture_with_routing_dispatcher_and_logs(
        state, platform, config, routing, dispatcher, None,
    )
}

fn start_daemon_input_capture_with_routing_dispatcher_and_logs<D>(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
    dispatcher: D,
    logs: Option<SharedDaemonLogBuffer>,
) -> Result<DaemonInputCaptureWorker, PlatformError>
where
    D: CoreActionDispatcher,
{
    start_daemon_input_capture_with_routing_dispatcher_logs_and_environment_inspector(
        state,
        platform,
        config,
        routing,
        dispatcher,
        logs,
        NoRuntimeEnvironmentInspector,
    )
}

fn start_daemon_input_capture_with_routing_dispatcher_logs_and_environment_inspector<D, E>(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
    dispatcher: D,
    logs: Option<SharedDaemonLogBuffer>,
    environment_inspector: E,
) -> Result<DaemonInputCaptureWorker, PlatformError>
where
    D: CoreActionDispatcher,
    E: RuntimeEnvironmentInspector + Send + 'static,
{
    let capture = platform.start_input_capture(config.input_capture)?;
    sync_capture_policy_with_state(&capture, &state)?;
    let running = Arc::new(AtomicBool::new(true));
    let worker_running = Arc::clone(&running);
    let thread = thread::Builder::new()
        .name("akraz-daemon-input-capture".to_string())
        .spawn(move || {
            run_daemon_input_capture_worker(DaemonInputCaptureWorkerRuntime {
                state,
                capture,
                running: worker_running,
                config,
                routing,
                dispatcher,
                logs,
                environment_inspector,
            })
        })
        .map_err(|error| {
            PlatformError::new(format!(
                "failed to start daemon input capture worker: {error}"
            ))
        })?;

    Ok(DaemonInputCaptureWorker::new(running, thread))
}

struct DaemonInputCaptureWorkerRuntime<D, E> {
    state: SharedRuntimeInputState,
    capture: InputCaptureSession,
    running: Arc<AtomicBool>,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
    dispatcher: D,
    logs: Option<SharedDaemonLogBuffer>,
    environment_inspector: E,
}

fn run_daemon_input_capture_worker<D, E>(
    runtime: DaemonInputCaptureWorkerRuntime<D, E>,
) -> Result<(), PlatformError>
where
    D: CoreActionDispatcher,
    E: RuntimeEnvironmentInspector,
{
    let DaemonInputCaptureWorkerRuntime {
        state,
        capture,
        running,
        config,
        routing,
        dispatcher,
        logs,
        mut environment_inspector,
    } = runtime;
    let idle_poll_interval = config.bounded_idle_poll_interval();
    let mut idle_watchdog =
        InputCaptureIdleWatchdog::new(config.bounded_idle_watchdog_timeout(), Instant::now());
    let mut power_resume_watchdog =
        PowerResumeWatchdog::new(DEFAULT_POWER_RESUME_POLL_GAP, Instant::now());
    let mut router = CapturedInputRouter::new(routing);

    environment_inspector.inspect(
        &state,
        &dispatcher,
        &mut router,
        logs.as_ref(),
        Instant::now(),
    )?;

    while running.load(Ordering::Acquire) {
        sync_capture_policy_with_state(&capture, &state)?;
        match capture.recv_timeout(idle_poll_interval) {
            Ok(event) => {
                let now = Instant::now();
                if power_resume_watchdog.record_poll(now) {
                    recover_runtime_after_system_resume_and_dispatch(
                        &state,
                        &dispatcher,
                        logs.as_ref(),
                    )?;
                }
                environment_inspector.inspect(
                    &state,
                    &dispatcher,
                    &mut router,
                    logs.as_ref(),
                    now,
                )?;
                idle_watchdog.record_progress(now);
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
                let now = Instant::now();
                sync_capture_policy_with_state(&capture, &state)?;
                if power_resume_watchdog.record_poll(now) {
                    recover_runtime_after_system_resume_and_dispatch(
                        &state,
                        &dispatcher,
                        logs.as_ref(),
                    )?;
                    idle_watchdog.record_progress(now);
                    continue;
                }
                environment_inspector.inspect(
                    &state,
                    &dispatcher,
                    &mut router,
                    logs.as_ref(),
                    now,
                )?;
                if let Some(action) = idle_watchdog.record_idle_poll(now) {
                    dispatch_input_capture_idle_watchdog_action(
                        &dispatcher,
                        logs.as_ref(),
                        action,
                    )?;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }

    capture.stop()
}

#[derive(Debug, Clone)]
struct PowerResumeWatchdog {
    max_poll_gap: Duration,
    last_poll_at: Instant,
}

impl PowerResumeWatchdog {
    fn new(max_poll_gap: Duration, now: Instant) -> Self {
        Self {
            max_poll_gap,
            last_poll_at: now,
        }
    }

    fn record_poll(&mut self, now: Instant) -> bool {
        let elapsed = now.duration_since(self.last_poll_at);
        self.last_poll_at = now;
        elapsed >= self.max_poll_gap
    }
}

#[derive(Debug, Clone)]
struct RuntimeEnvironmentWatchdog {
    poll_interval: Duration,
    last_poll_at: Instant,
    permissions_available: bool,
}

impl RuntimeEnvironmentWatchdog {
    fn new(poll_interval: Duration, now: Instant) -> Self {
        Self {
            poll_interval,
            last_poll_at: now.checked_sub(poll_interval).unwrap_or(now),
            permissions_available: true,
        }
    }

    fn record_poll(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last_poll_at) < self.poll_interval {
            return false;
        }

        self.last_poll_at = now;
        true
    }

    fn record_permissions_available(&mut self, available: bool) -> bool {
        let permission_lost = self.permissions_available && !available;
        self.permissions_available = available;
        permission_lost
    }
}

trait RuntimeEnvironmentInspector {
    fn inspect(
        &mut self,
        state: &SharedRuntimeInputState,
        dispatcher: &impl CoreActionDispatcher,
        router: &mut CapturedInputRouter,
        logs: Option<&SharedDaemonLogBuffer>,
        now: Instant,
    ) -> Result<(), PlatformError>;
}

#[derive(Debug, Clone, Copy)]
struct NoRuntimeEnvironmentInspector;

impl RuntimeEnvironmentInspector for NoRuntimeEnvironmentInspector {
    fn inspect(
        &mut self,
        _state: &SharedRuntimeInputState,
        _dispatcher: &impl CoreActionDispatcher,
        _router: &mut CapturedInputRouter,
        _logs: Option<&SharedDaemonLogBuffer>,
        _now: Instant,
    ) -> Result<(), PlatformError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PlatformRuntimeEnvironmentInspector<P> {
    platform: P,
    watchdog: RuntimeEnvironmentWatchdog,
}

impl<P> PlatformRuntimeEnvironmentInspector<P> {
    fn new(platform: P, now: Instant) -> Self {
        Self {
            platform,
            watchdog: RuntimeEnvironmentWatchdog::new(DEFAULT_RUNTIME_ENVIRONMENT_POLL_GAP, now),
        }
    }
}

impl<P> RuntimeEnvironmentInspector for PlatformRuntimeEnvironmentInspector<P>
where
    P: PlatformAdapter,
{
    fn inspect(
        &mut self,
        state: &SharedRuntimeInputState,
        dispatcher: &impl CoreActionDispatcher,
        router: &mut CapturedInputRouter,
        logs: Option<&SharedDaemonLogBuffer>,
        now: Instant,
    ) -> Result<(), PlatformError> {
        inspect_runtime_environment_and_recover(
            state,
            &self.platform,
            dispatcher,
            router,
            &mut self.watchdog,
            logs,
            now,
        )
    }
}

#[derive(Debug, Clone)]
struct InputCaptureIdleWatchdog {
    idle_timeout: Duration,
    last_progress_at: Instant,
    idle_reported: bool,
}

impl InputCaptureIdleWatchdog {
    fn new(idle_timeout: Duration, now: Instant) -> Self {
        Self {
            idle_timeout,
            last_progress_at: now,
            idle_reported: false,
        }
    }

    fn record_progress(&mut self, now: Instant) {
        self.last_progress_at = now;
        self.idle_reported = false;
    }

    fn record_idle_poll(&mut self, now: Instant) -> Option<CoreAction> {
        if self.idle_reported || now.duration_since(self.last_progress_at) < self.idle_timeout {
            return None;
        }

        self.idle_reported = true;
        Some(CoreAction::InputCaptureIdle)
    }
}

fn recover_runtime_after_system_resume_and_dispatch(
    state: &SharedRuntimeInputState,
    dispatcher: &impl CoreActionDispatcher,
    logs: Option<&SharedDaemonLogBuffer>,
) -> Result<(), PlatformError> {
    recover_runtime_after_interrupt_and_dispatch(
        state,
        dispatcher,
        logs,
        RuntimeRecoveryInterrupt::SystemResumed,
    )
}

fn recover_runtime_after_interrupt_and_dispatch(
    state: &SharedRuntimeInputState,
    dispatcher: &impl CoreActionDispatcher,
    logs: Option<&SharedDaemonLogBuffer>,
    interrupt: RuntimeRecoveryInterrupt,
) -> Result<(), PlatformError> {
    let mut state = state
        .lock()
        .map_err(|_| PlatformError::new("daemon runtime state is unavailable"))?;

    recover_runtime_state_after_interrupt_and_dispatch(&mut state, dispatcher, logs, interrupt)
}

fn recover_runtime_state_after_interrupt_and_dispatch(
    state: &mut RuntimeInputState,
    dispatcher: &impl CoreActionDispatcher,
    logs: Option<&SharedDaemonLogBuffer>,
    interrupt: RuntimeRecoveryInterrupt,
) -> Result<(), PlatformError> {
    match state
        .apply_event(interrupt.runtime_event())
        .map_err(|error| recovery_interrupt_transition_error(interrupt, error))
    {
        Ok(actions) => {
            record_daemon_event(
                logs,
                DaemonLogLevel::Warn,
                interrupt.success_log_event(),
                interrupt.success_log_message(),
            );
            dispatch_core_action_batch(dispatcher, actions)
        }
        Err(error) => {
            record_daemon_event(
                logs,
                DaemonLogLevel::Error,
                interrupt.failure_log_event(),
                interrupt.failure_log_message(),
            );
            Err(error)
        }
    }
}

fn dispatch_input_capture_idle_watchdog_action(
    dispatcher: &impl CoreActionDispatcher,
    logs: Option<&SharedDaemonLogBuffer>,
    action: CoreAction,
) -> Result<(), PlatformError> {
    record_daemon_event(
        logs,
        DaemonLogLevel::Warn,
        "input.capture.idle",
        "Input capture made no progress before the watchdog timeout.",
    );
    dispatch_core_action_batch(dispatcher, vec![action])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeRecoveryInterrupt {
    ScreenLayoutChanged,
    PermissionLost,
    SystemResumed,
}

impl RuntimeRecoveryInterrupt {
    fn runtime_event(self) -> RuntimeEvent {
        match self {
            Self::ScreenLayoutChanged => RuntimeEvent::ScreenLayoutChanged,
            Self::PermissionLost => RuntimeEvent::PermissionLost,
            Self::SystemResumed => RuntimeEvent::SystemResumed,
        }
    }

    fn success_log_event(self) -> &'static str {
        match self {
            Self::ScreenLayoutChanged => "input.capture.layoutChanged",
            Self::PermissionLost => "input.capture.permissionLost",
            Self::SystemResumed => "input.capture.resume",
        }
    }

    fn success_log_message(self) -> &'static str {
        match self {
            Self::ScreenLayoutChanged => "Input capture recovered after the screen layout changed.",
            Self::PermissionLost => "Input capture recovered after platform input permission loss.",
            Self::SystemResumed => {
                "Input capture recovered after a system resume or long poll gap."
            }
        }
    }

    fn failure_log_event(self) -> &'static str {
        match self {
            Self::ScreenLayoutChanged => "input.capture.layoutChanged.failed",
            Self::PermissionLost => "input.capture.permissionLost.failed",
            Self::SystemResumed => "input.capture.resume.failed",
        }
    }

    fn failure_log_message(self) -> &'static str {
        match self {
            Self::ScreenLayoutChanged => {
                "Input capture recovery failed after a screen layout change."
            }
            Self::PermissionLost => {
                "Input capture recovery failed after platform input permission loss."
            }
            Self::SystemResumed => {
                "Input capture recovery failed after a system resume or long poll gap."
            }
        }
    }

    fn error_context(self) -> &'static str {
        match self {
            Self::ScreenLayoutChanged => "screen layout change",
            Self::PermissionLost => "platform input permission loss",
            Self::SystemResumed => "system resume",
        }
    }
}

fn inspect_runtime_environment_and_recover(
    state: &SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    dispatcher: &impl CoreActionDispatcher,
    router: &mut CapturedInputRouter,
    watchdog: &mut RuntimeEnvironmentWatchdog,
    logs: Option<&SharedDaemonLogBuffer>,
    now: Instant,
) -> Result<(), PlatformError> {
    if !watchdog.record_poll(now) {
        return Ok(());
    }

    match platform.read_desktop_geometry() {
        Ok(geometry) => {
            if geometry.virtual_screen_bounds != router.screen_layout.local_bounds {
                recover_runtime_after_interrupt_and_dispatch(
                    state,
                    dispatcher,
                    logs,
                    RuntimeRecoveryInterrupt::ScreenLayoutChanged,
                )?;
                router.current_pointer = geometry.pointer_position;
                router.screen_layout = ScreenLayout::new(
                    geometry.virtual_screen_bounds,
                    router.screen_layout.edge_bindings.clone(),
                );
            }
        }
        Err(_) => record_daemon_event(
            logs,
            DaemonLogLevel::Warn,
            "input.capture.layoutProbe.failed",
            "Input capture screen layout probe failed.",
        ),
    }

    match platform.probe_capabilities() {
        Ok(capabilities) => {
            let available = required_input_capabilities_available(&capabilities);
            if watchdog.record_permissions_available(available) {
                recover_runtime_after_interrupt_and_dispatch(
                    state,
                    dispatcher,
                    logs,
                    RuntimeRecoveryInterrupt::PermissionLost,
                )?;
            }
        }
        Err(_) => record_daemon_event(
            logs,
            DaemonLogLevel::Warn,
            "input.capture.permissionProbe.failed",
            "Input capture permission probe failed.",
        ),
    }

    Ok(())
}

fn required_input_capabilities_available(capabilities: &PlatformCapabilities) -> bool {
    capabilities.can_capture_pointer
        && capabilities.can_capture_keyboard
        && capabilities.can_inject_pointer
        && capabilities.can_inject_keyboard
}

fn recovery_interrupt_transition_error(
    interrupt: RuntimeRecoveryInterrupt,
    error: CoreTransitionError,
) -> PlatformError {
    PlatformError::new(format!(
        "failed to recover after {}: {error}",
        interrupt.error_context()
    ))
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
    build_daemon_status_with_peer_sessions(state, platform, &ManagedPeerSessionTransport::new())
}

/// Build a `daemon.status` result including the managed peer-session snapshot.
pub fn build_daemon_status_with_peer_sessions(
    state: &RuntimeInputState,
    platform: &impl PlatformAdapter,
    peer_sessions: &ManagedPeerSessionTransport,
) -> Result<DaemonStatus, PlatformError> {
    let capabilities = platform.probe_capabilities()?;

    Ok(DaemonStatus {
        daemon_version: DAEMON_VERSION.to_string(),
        mode: state.mode().into(),
        protocol: ProtocolVersionSnapshot::from(ProtocolVersion::CURRENT),
        peers: active_peer_statuses(peer_sessions)?,
        capabilities: IpcPlatformCapabilities::from(capabilities),
    })
}

/// Build a sanitized `diagnostics.screenTopology` result from the selected platform adapter.
pub fn build_diagnostics_screen_topology(
    platform: &impl PlatformAdapter,
) -> Result<DiagnosticsScreenTopology, PlatformError> {
    let geometry = platform.read_desktop_geometry()?;

    Ok(DiagnosticsScreenTopology {
        pointer_position: geometry.pointer_position.into(),
        virtual_screen_bounds: geometry.virtual_screen_bounds.into(),
        monitors: geometry.monitors.into_iter().map(Into::into).collect(),
    })
}

/// Build a sanitized `diagnostics.keyboardLayout` result from the selected platform adapter.
pub fn build_diagnostics_keyboard_layout(
    platform: &impl PlatformAdapter,
) -> Result<DiagnosticsKeyboardLayout, PlatformError> {
    platform.read_keyboard_layout().map(Into::into)
}

/// Convert resolved discovery results into candidates that may be used for `session.connect`.
pub fn build_peer_session_discovery_candidates(
    local_device_id: &DeviceId,
    discovered_peers: &[DiscoveredPeer],
    trusted_peers: &[TrustedPeerIdentity],
) -> Vec<DiscoverySessionCandidate> {
    let filter = DiscoveryPeerFilter {
        local_device_id: Some(local_device_id.as_str().to_string()),
        required_capabilities: peer_session_capabilities(),
        blocked_device_ids: BTreeSet::new(),
    };

    build_discovery_session_candidates(discovered_peers, &filter, trusted_peers)
}

/// Convert discovery candidates into the local IPC result contract.
pub fn build_session_discovery_candidates_result(
    candidates: &[DiscoverySessionCandidate],
) -> SessionDiscoveryCandidatesResult {
    SessionDiscoveryCandidatesResult {
        candidates: candidates
            .iter()
            .map(|candidate| SessionDiscoveryCandidate {
                peer_id: candidate.peer_id.clone(),
                display_name: candidate.display_name.clone(),
                fingerprint: candidate.fingerprint.clone(),
                trusted: candidate.trusted,
                address: candidate.address.to_string(),
                build_version: candidate.build_version.clone(),
                capabilities: candidate.capabilities,
            })
            .collect(),
    }
}

/// Handle one local IPC JSON-RPC request line.
pub fn handle_ipc_request_line(
    state: &mut RuntimeInputState,
    platform: &impl PlatformAdapter,
    dispatcher: &impl CoreActionDispatcher,
    line: &str,
) -> Result<String, DaemonIpcError> {
    handle_ipc_request_line_with_peer_sessions(
        state,
        platform,
        dispatcher,
        &ManagedPeerSessionTransport::new(),
        line,
        None,
        None,
    )
}

/// Handle one local IPC JSON-RPC request line with a managed peer-session transport.
pub fn handle_ipc_request_line_with_peer_sessions(
    state: &mut RuntimeInputState,
    platform: &impl PlatformAdapter,
    dispatcher: &impl CoreActionDispatcher,
    peer_sessions: &ManagedPeerSessionTransport,
    line: &str,
    logs: Option<&SharedDaemonLogBuffer>,
    shutdown_requested: Option<&AtomicBool>,
) -> Result<String, DaemonIpcError> {
    match parse_request_line(line) {
        Ok(request) => handle_ipc_request(
            state,
            platform,
            dispatcher,
            peer_sessions,
            logs,
            shutdown_requested,
            request,
        ),
        Err(failure) => encode_response(&failure),
    }
}

fn handle_ipc_request(
    state: &mut RuntimeInputState,
    platform: &impl PlatformAdapter,
    dispatcher: &impl CoreActionDispatcher,
    peer_sessions: &ManagedPeerSessionTransport,
    logs: Option<&SharedDaemonLogBuffer>,
    shutdown_requested: Option<&AtomicBool>,
    request: IpcRequest,
) -> Result<String, DaemonIpcError> {
    match request {
        IpcRequest::DaemonStatus(request) => {
            record_daemon_event(
                logs,
                DaemonLogLevel::Info,
                "daemon.status",
                "Daemon status requested.",
            );
            match build_daemon_status_with_peer_sessions(state, platform, peer_sessions) {
                Ok(status) => encode_response(&JsonRpcSuccess::new(request.id, status)),
                Err(error) => encode_platform_error(request.id, "daemon status unavailable", error),
            }
        }
        IpcRequest::PermissionsProbe(request) => match build_permissions_probe(platform) {
            Ok(probe) => {
                if !probe.issues.is_empty()
                    && let Err(error) = recover_runtime_state_after_interrupt_and_dispatch(
                        state,
                        dispatcher,
                        logs,
                        RuntimeRecoveryInterrupt::PermissionLost,
                    )
                {
                    return encode_platform_error(
                        request.id,
                        "permission recovery unavailable",
                        error,
                    );
                }
                record_daemon_event(
                    logs,
                    DaemonLogLevel::Info,
                    "permissions.probe",
                    "Permissions probe requested.",
                );
                encode_response(&JsonRpcSuccess::new(request.id, probe))
            }
            Err(error) => {
                record_daemon_event(
                    logs,
                    DaemonLogLevel::Warn,
                    "permissions.probe.failed",
                    "Permissions probe failed.",
                );
                encode_platform_error(request.id, "permissions probe unavailable", error)
            }
        },
        IpcRequest::DiagnosticsScreenTopology(request) => {
            match build_diagnostics_screen_topology(platform) {
                Ok(topology) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Info,
                        "diagnostics.screenTopology",
                        "Screen topology diagnostics requested.",
                    );
                    encode_response(&JsonRpcSuccess::new(request.id, topology))
                }
                Err(error) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Warn,
                        "diagnostics.screenTopology.failed",
                        "Screen topology diagnostics failed.",
                    );
                    encode_platform_error(request.id, "screen topology unavailable", error)
                }
            }
        }
        IpcRequest::DiagnosticsKeyboardLayout(request) => {
            match build_diagnostics_keyboard_layout(platform) {
                Ok(keyboard_layout) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Info,
                        "diagnostics.keyboardLayout",
                        "Keyboard layout diagnostics requested.",
                    );
                    encode_response(&JsonRpcSuccess::new(request.id, keyboard_layout))
                }
                Err(error) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Warn,
                        "diagnostics.keyboardLayout.failed",
                        "Keyboard layout diagnostics failed.",
                    );
                    encode_platform_error(request.id, "keyboard layout unavailable", error)
                }
            }
        }
        IpcRequest::DaemonLogsTail(request) => {
            record_daemon_event(
                logs,
                DaemonLogLevel::Info,
                "daemon.logs.tail",
                "Daemon logs tail requested.",
            );
            let entries = daemon_log_tail(logs, request.params.limit);
            encode_response(&JsonRpcSuccess::new(request.id, DaemonLogsTail { entries }))
        }
        IpcRequest::DaemonShutdown(request) => {
            let Some(shutdown_requested) = shutdown_requested else {
                record_daemon_event(
                    logs,
                    DaemonLogLevel::Error,
                    "daemon.shutdown.failed",
                    "Daemon shutdown failed.",
                );
                return encode_platform_error(
                    request.id,
                    "daemon shutdown unavailable",
                    PlatformError::new("shutdown control flag is unavailable"),
                );
            };
            match shutdown_daemon(state, dispatcher, peer_sessions) {
                Ok(result) => {
                    shutdown_requested.store(true, Ordering::Release);
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Warn,
                        "daemon.shutdown",
                        "Daemon shutdown requested.",
                    );
                    encode_response(&JsonRpcSuccess::new(request.id, result))
                }
                Err(error) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Error,
                        "daemon.shutdown.failed",
                        "Daemon shutdown failed.",
                    );
                    encode_platform_error(request.id, "daemon shutdown unavailable", error)
                }
            }
        }
        IpcRequest::InputReleaseAll(request) => {
            match recover_local_control_and_release_inputs(state, dispatcher) {
                Ok(result) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Warn,
                        "input.releaseAll",
                        "Input release requested.",
                    );
                    encode_response(&JsonRpcSuccess::new(request.id, result))
                }
                Err(error) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Error,
                        "input.releaseAll.failed",
                        "Input release failed.",
                    );
                    encode_platform_error(request.id, "input release unavailable", error)
                }
            }
        }
        IpcRequest::SessionConnect(request) => {
            match connect_peer_session(&request.params, peer_sessions) {
                Ok(result) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Info,
                        "session.connect",
                        "Peer session connect requested.",
                    );
                    encode_response(&JsonRpcSuccess::new(request.id, result))
                }
                Err(error) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Warn,
                        "session.connect.failed",
                        "Peer session connect failed.",
                    );
                    encode_platform_error(request.id, "session connect unavailable", error)
                }
            }
        }
        IpcRequest::SessionDiscoveryCandidates(request) => {
            record_daemon_event(
                logs,
                DaemonLogLevel::Info,
                "session.discoveryCandidates",
                "Peer session discovery candidates requested.",
            );
            encode_response(&JsonRpcSuccess::new(
                request.id,
                build_session_discovery_candidates_result(&[]),
            ))
        }
        IpcRequest::SessionDisconnect(request) => {
            match disconnect_peer_session(state, dispatcher, peer_sessions) {
                Ok(result) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Info,
                        "session.disconnect",
                        "Peer session disconnect requested.",
                    );
                    encode_response(&JsonRpcSuccess::new(request.id, result))
                }
                Err(error) => {
                    record_daemon_event(
                        logs,
                        DaemonLogLevel::Warn,
                        "session.disconnect.failed",
                        "Peer session disconnect failed.",
                    );
                    encode_platform_error(request.id, "session disconnect unavailable", error)
                }
            }
        }
    }
}

fn shutdown_daemon(
    state: &mut RuntimeInputState,
    dispatcher: &impl CoreActionDispatcher,
    peer_sessions: &ManagedPeerSessionTransport,
) -> Result<DaemonShutdownResult, PlatformError> {
    let active_session_before_recovery = peer_sessions.active_session()?;
    let recovery = recover_local_control_and_release_inputs(state, dispatcher)?;
    let detached_session = peer_sessions
        .disconnect_session()?
        .or(active_session_before_recovery);

    Ok(DaemonShutdownResult {
        requested: true,
        released_inputs: recovery.released,
        disconnected_peer_session: detached_session.is_some(),
        mode: recovery.mode,
    })
}

fn record_daemon_event(
    logs: Option<&SharedDaemonLogBuffer>,
    level: DaemonLogLevel,
    event: &'static str,
    message: &'static str,
) {
    let Some(logs) = logs else {
        return;
    };
    if let Ok(mut buffer) = logs.lock() {
        buffer.record(level, event, message);
    }
}

fn daemon_log_tail(
    logs: Option<&SharedDaemonLogBuffer>,
    requested_limit: Option<usize>,
) -> Vec<DaemonLogEntry> {
    let Some(logs) = logs else {
        return Vec::new();
    };
    let limit = requested_limit.unwrap_or(DEFAULT_DAEMON_LOGS_TAIL_LIMIT);
    match logs.lock() {
        Ok(buffer) => buffer.tail(limit),
        Err(_) => Vec::new(),
    }
}

/// Connect the managed peer-session transport to a remote peer.
pub fn connect_peer_session(
    params: &SessionConnectParams,
    peer_sessions: &ManagedPeerSessionTransport,
) -> Result<SessionConnectResult, PlatformError> {
    if peer_sessions.is_connected()? {
        return Err(PlatformError::new(
            "managed peer session already has an active peer",
        ));
    }

    let peer_id = parse_non_empty_peer_id(&params.peer_id)?;
    let local_device_id = parse_non_empty_device_id(&params.local_device_id)?;
    let address = parse_peer_session_address(&params.address)?;

    peer_sessions.connect_session(peer_id, local_device_id, address)?;

    let snapshot = peer_sessions
        .active_session()?
        .ok_or_else(|| PlatformError::new("managed peer session was not attached"))?;

    Ok(SessionConnectResult {
        connected: true,
        session: session_status_from_snapshot(&snapshot, true),
    })
}

/// Disconnect the managed peer session after recovering local input control.
pub fn disconnect_peer_session(
    state: &mut RuntimeInputState,
    dispatcher: &impl CoreActionDispatcher,
    peer_sessions: &ManagedPeerSessionTransport,
) -> Result<SessionDisconnectResult, PlatformError> {
    let active_session_before_recovery = peer_sessions.active_session()?;
    let recovery = recover_local_control_and_release_inputs(state, dispatcher)?;
    let detached_session = peer_sessions
        .disconnect_session()?
        .or(active_session_before_recovery);

    Ok(SessionDisconnectResult {
        disconnected: detached_session.is_some(),
        session: detached_session
            .as_ref()
            .map(|session| session_status_from_snapshot(session, false)),
        mode: recovery.mode,
    })
}

fn active_peer_statuses(
    peer_sessions: &ManagedPeerSessionTransport,
) -> Result<Vec<PeerStatus>, PlatformError> {
    let mut statuses = Vec::new();
    if let Some(session) = peer_sessions.active_session()? {
        let display_name = peer_sessions
            .trusted_peer_display_name(&session.peer_id)?
            .unwrap_or_else(|| session.peer_id.as_str().to_string());

        statuses.push(PeerStatus {
            display_name,
            peer_id: session.peer_id.as_str().to_string(),
            connected: true,
            local_device_id: Some(session.local_device_id.as_str().to_string()),
            address: Some(session.address.to_string()),
        });
    }

    Ok(statuses)
}

fn session_status_from_snapshot(
    snapshot: &ManagedPeerSessionSnapshot,
    connected: bool,
) -> SessionStatus {
    SessionStatus {
        peer_id: snapshot.peer_id.as_str().to_string(),
        local_device_id: snapshot.local_device_id.as_str().to_string(),
        address: snapshot.address.to_string(),
        connected,
    }
}

fn parse_non_empty_peer_id(value: &str) -> Result<PeerId, PlatformError> {
    let value = require_non_empty_session_value("peerId", value)?;
    Ok(PeerId::new(value))
}

fn parse_non_empty_device_id(value: &str) -> Result<DeviceId, PlatformError> {
    let value = require_non_empty_session_value("localDeviceId", value)?;
    Ok(DeviceId::new(value))
}

fn parse_peer_session_address(value: &str) -> Result<SocketAddr, PlatformError> {
    let value = require_non_empty_session_value("address", value)?;
    value
        .parse::<SocketAddr>()
        .map_err(|error| PlatformError::new(format!("invalid peer session address: {error}")))
}

fn require_non_empty_session_value<'a>(
    field: &'static str,
    value: &'a str,
) -> Result<&'a str, PlatformError> {
    let value = value.trim();
    if value.is_empty() {
        Err(PlatformError::new(format!(
            "session connect field {field} must not be empty"
        )))
    } else {
        Ok(value)
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
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use akraz_core::{
        CapturedInputEvent, ControlMode, CoreAction, DEFAULT_PANIC_HOTKEY_KEY, DeviceId,
        EdgeCrossing, InjectedInputEvent, LogicalPoint, LogicalRect, LogicalSize, MouseButton,
        PeerId, PhysicalKey, PressState, RuntimeEvent, RuntimeInputState, ScreenEdge,
        ScreenEdgeBinding, ScreenLayout, SessionId,
    };
    use akraz_discovery::{DiscoveredPeer, DiscoverySessionCandidate, DiscoveryTxtRecord};
    use akraz_identity::{
        DeviceIdentity, Ed25519IdentityKey, Ed25519PublicKey, FileIdentityStore, LocalIdentity,
        TrustedPeer, TrustedPeerIdentity, fingerprint_for_public_key,
    };
    use akraz_ipc::{
        ControlModeSnapshot, DaemonLogLevel, DaemonLogsTail, DaemonLogsTailParams,
        DaemonShutdownParams, DaemonShutdownResult, DaemonStatus, DaemonStatusParams,
        DiagnosticsKeyboardLayout, DiagnosticsKeyboardLayoutParams, DiagnosticsScreenTopology,
        DiagnosticsScreenTopologyParams, InputReleaseAllParams, InputReleaseAllResult, IpcEndpoint,
        IpcPlatformCapabilities, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcServer,
        METHOD_DAEMON_LOGS_TAIL, METHOD_DAEMON_SHUTDOWN, METHOD_DAEMON_STATUS,
        METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT, METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
        METHOD_INPUT_RELEASE_ALL, METHOD_PERMISSIONS_PROBE, METHOD_SESSION_CONNECT,
        METHOD_SESSION_DISCOVERY_CANDIDATES, OsLocalIpcClient, PeerStatus, PermissionsProbe,
        PermissionsProbeParams, SessionConnectParams, SessionDisconnectResult,
        SessionDiscoveryCandidatesParams, SessionDiscoveryCandidatesResult, SessionStatus,
        call_json_rpc, to_json_line,
    };
    use akraz_platform::{
        DesktopGeometry, DesktopMonitor, FakePlatformAdapter, InputCaptureConfig,
        InputCapturePolicy, KeyboardLayoutSnapshot, PlatformAdapter, PlatformCapabilities,
        PlatformError,
    };
    use akraz_protocol::{
        AuthTranscript, CapabilityFlags, HANDSHAKE_NONCE_LEN, PeerRole, ProtocolVersion,
        SessionReady, TLS_EXPORTER_LEN,
    };
    use serde_json::json;

    use super::{
        CapturedInputRouter, CoreActionDispatcher, DAEMON_VERSION, DaemonInputCaptureConfig,
        DaemonInputRoutingConfig, DaemonIpcRunConfig, DaemonIpcServer, DaemonPeerTransport,
        DaemonTransportCommand, InputCaptureIdleWatchdog, JSONRPC_DAEMON_ERROR,
        LocalPlatformCoreActionDispatcher, LoopbackPeerTransport, ManagedPeerSessionSnapshot,
        ManagedPeerSessionTransport, NoopCoreActionDispatcher, PeerSessionReconnectBackoff,
        PeerTransportCommandExecution, PeerTransportCommandPayload, PeerTransportInputEvent,
        PeerTransportMessage, PeerTransportProtocolVersion, PeerTransportSessionFrame,
        PowerResumeWatchdog, RuntimeEnvironmentWatchdog, TcpPeerSessionHeartbeatWorker,
        TcpPeerSessionStreamState, TcpPeerSessionTransport, TcpPeerTransport,
        TransportCoreActionDispatcher, apply_routed_capture_event_to_state, build_daemon_status,
        build_daemon_status_with_peer_sessions, build_diagnostics_keyboard_layout,
        build_diagnostics_screen_topology, build_peer_session_discovery_candidates,
        build_permissions_probe, build_session_discovery_candidates_result, connect_peer_session,
        disconnect_peer_session, dispatch_input_capture_idle_watchdog_action, drain_capture_events,
        execute_authenticated_peer_transport_session_stream_until_closed,
        execute_paired_tcp_peer_transport_session_until_closed_with_timeout,
        execute_peer_transport_command, execute_peer_transport_session_stream_until_closed,
        handle_ipc_request_line, input_capture_policy_for_control_mode,
        inspect_runtime_environment_and_recover, recover_local_control_and_release_inputs,
        recover_runtime_after_system_resume_and_dispatch, serve_daemon_ipc,
        serve_tcp_peer_transport_commands, serve_tcp_peer_transport_session,
        serve_tcp_peer_transport_session_and_execute,
        serve_tcp_peer_transport_session_and_execute_until_closed,
        serve_tcp_peer_transport_session_and_execute_until_closed_with_timeout,
        shared_daemon_log_buffer, shared_runtime_state, start_daemon_input_capture,
        start_daemon_input_capture_with_edge_bindings, start_daemon_input_capture_with_routing,
        start_daemon_input_capture_with_routing_and_dispatcher,
        start_monitored_daemon_input_capture_with_edge_bindings_dispatcher_and_logs,
        sync_capture_policy_with_state,
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

    fn topology_or_panic(platform: &FakePlatformAdapter) -> akraz_ipc::DiagnosticsScreenTopology {
        match build_diagnostics_screen_topology(platform) {
            Ok(topology) => topology,
            Err(error) => panic!("expected screen topology: {error}"),
        }
    }

    fn keyboard_layout_or_panic(
        platform: &FakePlatformAdapter,
    ) -> akraz_ipc::DiagnosticsKeyboardLayout {
        match build_diagnostics_keyboard_layout(platform) {
            Ok(keyboard_layout) => keyboard_layout,
            Err(error) => panic!("expected keyboard layout: {error}"),
        }
    }

    fn korean_keyboard_layout() -> KeyboardLayoutSnapshot {
        KeyboardLayoutSnapshot {
            source: "foregroundWindowThread".to_string(),
            layout_id: "0x0000000004120412".to_string(),
            language_id: "0x0412".to_string(),
            layout_name: Some("00000412".to_string()),
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
            monitors: vec![DesktopMonitor {
                id: "primary".to_string(),
                bounds: LogicalRect {
                    origin: LogicalPoint { x: 0, y: 0 },
                    size: LogicalSize {
                        width: 1920,
                        height: 1080,
                    },
                },
                scale_factor_percent: Some(100),
                is_primary: true,
            }],
        }
    }

    fn peer_session_hello_frame() -> PeerTransportSessionFrame {
        PeerTransportSessionFrame::Hello {
            protocol: PeerTransportProtocolVersion {
                major: akraz_protocol::ProtocolVersion::CURRENT.major,
                minor: akraz_protocol::ProtocolVersion::CURRENT.minor,
            },
            device_id: "windows-desktop".to_string(),
            peer_id: "right-peer".to_string(),
            nonce: Some([1; HANDSHAKE_NONCE_LEN]),
            capabilities: super::peer_session_capabilities(),
        }
    }

    fn peer_session_frame_line(frame: &PeerTransportSessionFrame) -> String {
        format!(
            "{}\n",
            serde_json::to_string(frame).expect("peer session frame JSON")
        )
    }

    fn peer_session_command_frame(
        sequence: u64,
        command: PeerTransportCommandPayload,
    ) -> PeerTransportSessionFrame {
        PeerTransportSessionFrame::Command { sequence, command }
    }

    fn peer_session_auth_fixture() -> (
        LocalIdentity<Ed25519IdentityKey>,
        TrustedPeer<Ed25519PublicKey>,
        AuthTranscript,
    ) {
        let secret_key = Ed25519IdentityKey::generate();
        let public_key = secret_key.public_key_bytes();
        let fingerprint = fingerprint_for_public_key(&public_key);
        let local = LocalIdentity::new(
            DeviceIdentity::new(
                "windows-desktop",
                "Windows Desktop",
                public_key,
                fingerprint.clone(),
            ),
            secret_key,
        );
        let trusted = TrustedPeer::new(
            TrustedPeerIdentity::new(
                "windows-desktop",
                "Windows Desktop",
                public_key,
                fingerprint,
                CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            ),
            Ed25519PublicKey::from_public_key_bytes(&public_key).expect("trusted public key"),
        );
        let transcript = AuthTranscript {
            local_device_id: "windows-desktop".to_string(),
            remote_device_id: "right-peer".to_string(),
            local_nonce: [1; HANDSHAKE_NONCE_LEN],
            remote_nonce: [2; HANDSHAKE_NONCE_LEN],
            protocol: ProtocolVersion::CURRENT,
            local_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            remote_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            role: PeerRole::Initiator,
            tls_exporter: [3; TLS_EXPORTER_LEN],
        };

        (local, trusted, transcript)
    }

    fn peer_session_pair_auth_fixture() -> (
        LocalIdentity<Ed25519IdentityKey>,
        TrustedPeer<Ed25519PublicKey>,
        TrustedPeer<Ed25519PublicKey>,
    ) {
        let source_secret = Ed25519IdentityKey::generate();
        let source_public = source_secret.public_key_bytes();
        let source_fingerprint = fingerprint_for_public_key(&source_public);
        let source = LocalIdentity::new(
            DeviceIdentity::new(
                "windows-desktop",
                "Windows Desktop",
                source_public,
                source_fingerprint.clone(),
            ),
            source_secret,
        );

        let target_secret = Ed25519IdentityKey::generate();
        let target_public = target_secret.public_key_bytes();
        let target_fingerprint = fingerprint_for_public_key(&target_public);
        let trusted_target = TrustedPeer::new(
            TrustedPeerIdentity::new(
                "right-peer",
                "Right Peer",
                target_public,
                target_fingerprint,
                super::peer_session_capabilities(),
            ),
            Ed25519PublicKey::from_public_key_bytes(&target_public).expect("target public key"),
        );
        let trusted_source = TrustedPeer::new(
            TrustedPeerIdentity::new(
                "windows-desktop",
                "Windows Desktop",
                source_public,
                source_fingerprint,
                super::peer_session_capabilities(),
            ),
            Ed25519PublicKey::from_public_key_bytes(&source_public).expect("source public key"),
        );

        (source, trusted_target, trusted_source)
    }

    fn peer_session_auth_proof_frame(
        local: &LocalIdentity<Ed25519IdentityKey>,
        transcript: &AuthTranscript,
    ) -> PeerTransportSessionFrame {
        PeerTransportSessionFrame::AuthProof {
            proof: local
                .sign_auth_proof(PeerRole::Initiator, transcript)
                .expect("auth proof"),
        }
    }

    fn peer_session_ready_frame() -> PeerTransportSessionFrame {
        PeerTransportSessionFrame::SessionReady {
            ready: SessionReady {
                session_id: "loopback-session".to_string(),
                sequence_base: 0,
                capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            },
        }
    }

    fn unique_identity_store_path(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);

        std::env::temp_dir().join(format!(
            "akraz-daemon-{label}-{}-{nanos}.json",
            std::process::id()
        ))
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

    fn discovered_peer(device_id: &str, capabilities: CapabilityFlags) -> DiscoveredPeer {
        DiscoveredPeer {
            instance_name: format!("{device_id}._akraz._tcp.local."),
            host_name: format!("{device_id}.local."),
            addresses: vec!["127.0.0.1".parse().expect("loopback discovery address")],
            port: 4455,
            txt: DiscoveryTxtRecord {
                version: 1,
                device_id: device_id.to_string(),
                capabilities,
                build_version: "0.5.6".to_string(),
            },
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
        assert_eq!(status.protocol.minor, 4);
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
    fn daemon_status_uses_trusted_peer_display_name_for_active_session() {
        let identity_store_path = unique_identity_store_path("trusted-peer-display-name");
        let _ = std::fs::remove_file(&identity_store_path);
        let identity_store = FileIdentityStore::new(&identity_store_path);
        identity_store
            .load_or_create("Windows Desktop")
            .expect("create local identity store");
        let (_, trusted_target, _) = peer_session_pair_auth_fixture();
        identity_store
            .save_trusted_peer(trusted_target.identity())
            .expect("save trusted target peer");

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&listener, 0));
        let peer_sessions = ManagedPeerSessionTransport::with_identity_store(
            &identity_store_path,
            "Windows Desktop",
        );
        let session = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");
        peer_sessions
            .attach_session(session)
            .expect("attach managed peer session");

        let status = build_daemon_status_with_peer_sessions(
            &RuntimeInputState::new(),
            &FakePlatformAdapter::default(),
            &peer_sessions,
        )
        .expect("daemon status with trusted peer display name");

        assert_eq!(
            status.peers,
            vec![PeerStatus {
                display_name: "Right Peer".to_string(),
                peer_id: "right-peer".to_string(),
                connected: true,
                local_device_id: Some("windows-desktop".to_string()),
                address: Some(address.to_string()),
            }]
        );

        drop(peer_sessions);
        assert!(
            server_thread
                .join()
                .expect("TCP peer session server")
                .is_ok()
        );
        let _ = std::fs::remove_file(&identity_store_path);
    }

    #[test]
    fn peer_session_discovery_candidates_apply_daemon_session_policy() {
        let trusted = TrustedPeerIdentity::new(
            "linux-laptop",
            "Linux Laptop",
            b"public-key".to_vec(),
            "AKRZ-TRUSTED",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let local = discovered_peer(
            "windows-desktop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let accepted = discovered_peer(
            "linux-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let missing_pointer = discovered_peer("keyboard-only", CapabilityFlags::KEYBOARD);

        let candidates = build_peer_session_discovery_candidates(
            &DeviceId::new("windows-desktop"),
            &[local, accepted, missing_pointer],
            &[trusted],
        );

        assert_eq!(
            candidates,
            vec![DiscoverySessionCandidate {
                peer_id: "linux-laptop".to_string(),
                display_name: "Linux Laptop".to_string(),
                fingerprint: Some("AKRZ-TRUSTED".to_string()),
                trusted: true,
                address: "127.0.0.1:4455".parse().expect("candidate address"),
                build_version: "0.5.6".to_string(),
                capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            }]
        );
    }

    #[test]
    fn session_discovery_candidates_result_uses_ipc_wire_shape() {
        let candidate = DiscoverySessionCandidate {
            peer_id: "linux-laptop".to_string(),
            display_name: "Linux Laptop".to_string(),
            fingerprint: Some("AKRZ-TRUSTED".to_string()),
            trusted: true,
            address: "127.0.0.1:4455".parse().expect("candidate address"),
            build_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        };

        let result = build_session_discovery_candidates_result(&[candidate]);

        assert_eq!(
            serde_json::to_value(result).expect("session discovery JSON"),
            json!({
                "candidates": [{
                    "peerId": "linux-laptop",
                    "displayName": "Linux Laptop",
                    "fingerprint": "AKRZ-TRUSTED",
                    "trusted": true,
                    "address": "127.0.0.1:4455",
                    "buildVersion": env!("CARGO_PKG_VERSION"),
                    "capabilities": 3
                }]
            })
        );
    }

    #[test]
    fn daemon_ipc_session_discovery_candidates_returns_empty_without_resolver() {
        let mut state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let dispatcher = NoopCoreActionDispatcher;
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_SESSION_DISCOVERY_CANDIDATES,
            SessionDiscoveryCandidatesParams::default(),
        );
        let request_line = to_json_line(&request).expect("session discovery request line");

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, &request_line) {
                Ok(line) => line,
                Err(error) => panic!("expected session discovery response: {error}"),
            };
        let response: JsonRpcSuccess<SessionDiscoveryCandidatesResult> =
            match serde_json::from_str(&response_line) {
                Ok(response) => response,
                Err(error) => panic!("expected session discovery success JSON: {error}"),
            };

        assert_eq!(
            response.result,
            SessionDiscoveryCandidatesResult {
                candidates: Vec::new()
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
    fn permissions_probe_recovers_remote_control_when_capability_is_missing() {
        let mut state = RuntimeInputState::new();
        state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-permission"),
            })
            .expect("remote entry confirmed");
        let platform = FakePlatformAdapter::new(PlatformCapabilities {
            can_capture_pointer: true,
            can_capture_keyboard: false,
            can_inject_pointer: true,
            can_inject_keyboard: true,
        });
        let dispatcher = RecordingCoreActionDispatcher::default();
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_PERMISSIONS_PROBE,
            PermissionsProbeParams::default(),
        );
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, &request_line) {
                Ok(line) => line,
                Err(error) => panic!("expected permissions probe response: {error}"),
            };
        let response: JsonRpcSuccess<PermissionsProbe> = match serde_json::from_str(&response_line)
        {
            Ok(response) => response,
            Err(error) => panic!("expected permissions probe response JSON: {error}"),
        };

        assert_eq!(response.id, "req_1");
        assert_eq!(response.result.issues.len(), 1);
        assert_eq!(
            response.result.issues[0].code,
            "capture_keyboard_unavailable"
        );
        assert_eq!(state.mode(), ControlMode::Local);
        assert_eq!(
            dispatcher.snapshot(),
            vec![
                CoreAction::ReleaseLocalInputs,
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-permission")),
                },
            ]
        );
    }

    #[test]
    fn diagnostics_screen_topology_reflects_desktop_geometry() {
        let platform = FakePlatformAdapter::default().with_desktop_geometry(DesktopGeometry {
            pointer_position: LogicalPoint { x: 1919, y: 540 },
            virtual_screen_bounds: LogicalRect {
                origin: LogicalPoint { x: -1920, y: 0 },
                size: LogicalSize {
                    width: 3840,
                    height: 1080,
                },
            },
            monitors: vec![
                DesktopMonitor {
                    id: "left".to_string(),
                    bounds: LogicalRect {
                        origin: LogicalPoint { x: -1920, y: 0 },
                        size: LogicalSize {
                            width: 1920,
                            height: 1080,
                        },
                    },
                    scale_factor_percent: Some(125),
                    is_primary: false,
                },
                DesktopMonitor {
                    id: "primary".to_string(),
                    bounds: LogicalRect {
                        origin: LogicalPoint { x: 0, y: 0 },
                        size: LogicalSize {
                            width: 1920,
                            height: 1080,
                        },
                    },
                    scale_factor_percent: Some(100),
                    is_primary: true,
                },
            ],
        });

        let topology = topology_or_panic(&platform);

        assert_eq!(topology.pointer_position.x, 1919);
        assert_eq!(topology.pointer_position.y, 540);
        assert_eq!(topology.virtual_screen_bounds.x, -1920);
        assert_eq!(topology.virtual_screen_bounds.y, 0);
        assert_eq!(topology.virtual_screen_bounds.width, 3840);
        assert_eq!(topology.virtual_screen_bounds.height, 1080);
        assert_eq!(topology.monitors.len(), 2);
        assert_eq!(topology.monitors[0].id, "left");
        assert_eq!(topology.monitors[0].scale_factor_percent, Some(125));
        assert!(topology.monitors[1].is_primary);
    }

    #[test]
    fn diagnostics_keyboard_layout_reflects_platform_input_locale() {
        let platform =
            FakePlatformAdapter::default().with_keyboard_layout(korean_keyboard_layout());

        let keyboard_layout = keyboard_layout_or_panic(&platform);

        assert_eq!(
            keyboard_layout,
            DiagnosticsKeyboardLayout {
                source: "foregroundWindowThread".to_string(),
                layout_id: "0x0000000004120412".to_string(),
                language_id: "0x0412".to_string(),
                layout_name: Some("00000412".to_string()),
            }
        );
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
    fn capture_idle_watchdog_reports_once_per_idle_period() {
        let now = Instant::now();
        let mut watchdog = InputCaptureIdleWatchdog::new(Duration::from_millis(20), now);

        assert_eq!(
            watchdog.record_idle_poll(now + Duration::from_millis(19)),
            None
        );
        assert_eq!(
            watchdog.record_idle_poll(now + Duration::from_millis(20)),
            Some(CoreAction::InputCaptureIdle)
        );
        assert_eq!(
            watchdog.record_idle_poll(now + Duration::from_millis(40)),
            None
        );
    }

    #[test]
    fn capture_idle_watchdog_rearms_after_progress() {
        let now = Instant::now();
        let mut watchdog = InputCaptureIdleWatchdog::new(Duration::from_millis(20), now);

        assert_eq!(
            watchdog.record_idle_poll(now + Duration::from_millis(20)),
            Some(CoreAction::InputCaptureIdle)
        );

        watchdog.record_progress(now + Duration::from_millis(25));

        assert_eq!(
            watchdog.record_idle_poll(now + Duration::from_millis(44)),
            None
        );
        assert_eq!(
            watchdog.record_idle_poll(now + Duration::from_millis(45)),
            Some(CoreAction::InputCaptureIdle)
        );
    }

    #[test]
    fn power_resume_watchdog_reports_only_long_poll_gaps() {
        let now = Instant::now();
        let mut watchdog = PowerResumeWatchdog::new(Duration::from_secs(5), now);

        assert!(!watchdog.record_poll(now + Duration::from_secs(4)));
        assert!(watchdog.record_poll(now + Duration::from_secs(9)));
        assert!(!watchdog.record_poll(now + Duration::from_secs(10)));
    }

    #[test]
    fn resume_recovery_logs_sanitized_watchdog_event() {
        let mut initial_state = RuntimeInputState::new();
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("secret-peer-id"),
            })
            .expect("remote entry request");
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("secret-session-id"),
            })
            .expect("remote entry confirmed");
        let state = shared_runtime_state(initial_state);
        let dispatcher = RecordingCoreActionDispatcher::default();
        let logs = shared_daemon_log_buffer(8);

        recover_runtime_after_system_resume_and_dispatch(&state, &dispatcher, Some(&logs))
            .expect("resume recovery");

        let entries = logs.lock().expect("daemon logs lock").tail(8);
        let encoded = serde_json::to_string(&entries).expect("daemon logs JSON");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, DaemonLogLevel::Warn);
        assert_eq!(entries[0].event, "input.capture.resume");
        assert!(
            dispatcher
                .snapshot()
                .contains(&CoreAction::ReleaseLocalInputs)
        );
        assert!(!encoded.contains("secret-peer-id"));
        assert!(!encoded.contains("secret-session-id"));
    }

    #[test]
    fn runtime_environment_watchdog_recovers_after_screen_layout_change() {
        let mut initial_state = RuntimeInputState::new();
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-layout"),
            })
            .expect("remote entry confirmed");
        let state = shared_runtime_state(initial_state);
        let platform = FakePlatformAdapter::default().with_desktop_geometry(DesktopGeometry {
            pointer_position: LogicalPoint { x: 2559, y: 720 },
            virtual_screen_bounds: LogicalRect {
                origin: LogicalPoint { x: 0, y: 0 },
                size: LogicalSize {
                    width: 2560,
                    height: 1440,
                },
            },
            monitors: vec![DesktopMonitor {
                id: "primary".to_string(),
                bounds: LogicalRect {
                    origin: LogicalPoint { x: 0, y: 0 },
                    size: LogicalSize {
                        width: 2560,
                        height: 1440,
                    },
                },
                scale_factor_percent: Some(100),
                is_primary: true,
            }],
        });
        let dispatcher = RecordingCoreActionDispatcher::default();
        let logs = shared_daemon_log_buffer(8);
        let now = Instant::now();
        let mut watchdog = RuntimeEnvironmentWatchdog::new(Duration::from_millis(1), now);
        let mut router = CapturedInputRouter::new(DaemonInputRoutingConfig {
            screen_layout: right_edge_layout(),
            initial_pointer: LogicalPoint { x: 1919, y: 540 },
        });

        inspect_runtime_environment_and_recover(
            &state,
            &platform,
            &dispatcher,
            &mut router,
            &mut watchdog,
            Some(&logs),
            now + Duration::from_millis(1),
        )
        .expect("runtime environment inspection");

        assert_eq!(
            state.lock().expect("runtime state lock").mode(),
            ControlMode::Local
        );
        assert_eq!(
            dispatcher.snapshot(),
            vec![
                CoreAction::ReleaseLocalInputs,
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-layout")),
                },
            ]
        );
        assert_eq!(router.current_pointer, LogicalPoint { x: 2559, y: 720 });
        assert_eq!(router.screen_layout.local_bounds.size.width, 2560);
        assert_eq!(router.screen_layout.local_bounds.size.height, 1440);

        let entries = logs.lock().expect("daemon logs lock").tail(8);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, DaemonLogLevel::Warn);
        assert_eq!(entries[0].event, "input.capture.layoutChanged");
    }

    #[test]
    fn capture_idle_recovery_logs_sanitized_watchdog_event() {
        let dispatcher = RecordingCoreActionDispatcher::default();
        let logs = shared_daemon_log_buffer(8);

        dispatch_input_capture_idle_watchdog_action(
            &dispatcher,
            Some(&logs),
            CoreAction::InputCaptureIdle,
        )
        .expect("idle watchdog dispatch");

        let entries = logs.lock().expect("daemon logs lock").tail(8);
        let encoded = serde_json::to_string(&entries).expect("daemon logs JSON");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, DaemonLogLevel::Warn);
        assert_eq!(entries[0].event, "input.capture.idle");
        assert!(
            dispatcher
                .snapshot()
                .contains(&CoreAction::InputCaptureIdle)
        );
        assert!(!encoded.contains("privateKey"));
        assert!(!encoded.contains("actualKeyInput"));
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
            .dispatch_core_actions(&[
                CoreAction::InputCaptureIdle,
                CoreAction::ReleaseLocalInputs,
                CoreAction::ReleaseAllInputs,
            ])
            .expect("transport command dispatch");

        assert_eq!(
            transport.snapshot().expect("loopback command snapshot"),
            vec![DaemonTransportCommand::ReleaseAllInputs]
        );
    }

    #[test]
    fn local_platform_dispatcher_delegates_idle_watchdog_without_local_release() {
        let platform = FakePlatformAdapter::default();
        let dispatcher = LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            RecordingCoreActionDispatcher::default(),
        );

        dispatcher
            .dispatch_core_actions(&[CoreAction::InputCaptureIdle])
            .expect("local platform dispatch");

        assert_eq!(
            platform.release_all_count().expect("local release count"),
            0
        );
        assert_eq!(
            dispatcher.next().snapshot(),
            vec![CoreAction::InputCaptureIdle]
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
            protocol: super::PeerTransportProtocolVersion { major: 1, minor: 4 },
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
    fn tcp_peer_session_transport_sends_heartbeat_frames_while_idle() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().map_err(|error| {
                PlatformError::new(format!("failed to accept test peer: {error}"))
            })?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .map_err(|error| {
                    PlatformError::new(format!("failed to configure test read timeout: {error}"))
                })?;
            let mut reader = std::io::BufReader::new(stream);
            let hello = super::read_peer_transport_session_hello(&mut reader)?;

            loop {
                match super::read_optional_peer_transport_session_frame(&mut reader)? {
                    Some(PeerTransportSessionFrame::Heartbeat { sequence }) => {
                        if sequence != 0 {
                            return Err(PlatformError::new(format!(
                                "test peer expected first heartbeat sequence 0, got {sequence}"
                            )));
                        }
                        return Ok::<_, PlatformError>(hello);
                    }
                    Some(PeerTransportSessionFrame::Command { .. }) => {}
                    Some(PeerTransportSessionFrame::Hello { .. }) => {
                        return Err(PlatformError::new(
                            "test peer received duplicate hello before heartbeat",
                        ));
                    }
                    Some(PeerTransportSessionFrame::AuthProof { .. }) => {
                        return Err(PlatformError::new(
                            "test peer received auth proof before heartbeat",
                        ));
                    }
                    Some(PeerTransportSessionFrame::SessionReady { .. }) => {
                        return Err(PlatformError::new(
                            "test peer received session ready before heartbeat",
                        ));
                    }
                    None => {
                        return Err(PlatformError::new(
                            "test peer closed before heartbeat was received",
                        ));
                    }
                }
            }
        });
        let transport = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");

        let hello = server_thread
            .join()
            .expect("TCP peer heartbeat server thread")
            .expect("TCP peer heartbeat");
        drop(transport);

        assert_eq!(hello.device_id, DeviceId::new("windows-desktop"));
        assert_eq!(hello.peer_id, PeerId::new("right-peer"));
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
    fn managed_peer_session_transport_detaches_when_stop_session_is_dispatched() {
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
        managed
            .dispatch_transport_command(DaemonTransportCommand::StopRemoteSession {
                session_id: Some(SessionId::new("session-resume")),
            })
            .expect("managed stop dispatch");

        assert_eq!(managed.active_session(), Ok(None));
        assert_eq!(managed.is_connected(), Ok(false));

        let session = server_thread
            .join()
            .expect("TCP peer session server thread")
            .expect("TCP peer session");
        assert_eq!(
            session.commands,
            vec![DaemonTransportCommand::StopRemoteSession {
                session_id: Some(SessionId::new("session-resume")),
            }]
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
    fn peer_session_reconnect_backoff_grows_and_resets() {
        let mut backoff = PeerSessionReconnectBackoff::default();
        let start = Instant::now();

        assert_eq!(backoff.record_failure(start), Duration::from_millis(250));
        assert_eq!(
            backoff.retry_after(start + Duration::from_millis(100)),
            Some(Duration::from_millis(150))
        );
        assert_eq!(
            backoff.retry_after(start + Duration::from_millis(250)),
            None
        );
        assert_eq!(
            backoff.record_failure(start + Duration::from_millis(250)),
            Duration::from_millis(500)
        );
        assert_eq!(
            backoff.record_failure(start + Duration::from_millis(750)),
            Duration::from_secs(1)
        );

        for attempt in 0..10 {
            let _ = backoff.record_failure(start + Duration::from_secs(2 + attempt));
        }
        assert_eq!(
            backoff.record_failure(start + Duration::from_secs(20)),
            Duration::from_secs(5)
        );

        backoff.reset();
        assert_eq!(backoff.retry_after(start + Duration::from_secs(21)), None);
        assert_eq!(
            backoff.record_failure(start + Duration::from_secs(21)),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn managed_peer_session_transport_delays_immediate_connect_retry_after_failure() {
        let identity_store_path = unique_identity_store_path("connect-backoff");
        let _ = std::fs::remove_file(&identity_store_path);
        let identity_store = FileIdentityStore::new(&identity_store_path);
        let local = identity_store
            .load_or_create("Windows Desktop")
            .expect("local identity")
            .into_local_identity();
        let (_, trusted_target, _) = peer_session_pair_auth_fixture();
        identity_store
            .save_trusted_peer(trusted_target.identity())
            .expect("save trusted target peer");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind temporary listener");
        let unavailable_address = listener.local_addr().expect("temporary listener address");
        drop(listener);
        let managed = ManagedPeerSessionTransport::with_identity_store(
            &identity_store_path,
            "Windows Desktop",
        );

        let first_error = managed
            .connect_session(
                PeerId::new("right-peer"),
                DeviceId::new(local.identity().device_id()),
                unavailable_address,
            )
            .expect_err("closed listener should fail connect");
        let retry_error = managed
            .connect_session(
                PeerId::new("right-peer"),
                DeviceId::new(local.identity().device_id()),
                unavailable_address,
            )
            .expect_err("immediate retry should be delayed");

        assert!(
            first_error
                .to_string()
                .contains("retry peer session connect after 250ms")
        );
        assert!(
            retry_error
                .to_string()
                .starts_with("peer session reconnect backoff active for ")
        );
        assert_eq!(managed.active_session(), Ok(None));

        let _ = std::fs::remove_file(&identity_store_path);
    }

    #[test]
    fn managed_peer_session_transport_delays_immediate_connect_retry_after_dispatch_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let address = listener.local_addr().expect("loopback listener address");
        let client = TcpStream::connect(address).expect("connect loopback client");
        let (_server, _) = listener.accept().expect("accept loopback client");
        client
            .shutdown(std::net::Shutdown::Both)
            .expect("shutdown loopback client");
        let managed = ManagedPeerSessionTransport::new();
        let broken_session = TcpPeerSessionTransport {
            peer_id: PeerId::new("right-peer"),
            local_device_id: DeviceId::new("windows-desktop"),
            address,
            stream: Arc::new(Mutex::new(TcpPeerSessionStreamState::new(client))),
            _heartbeat_worker: TcpPeerSessionHeartbeatWorker::start(
                PeerId::new("right-peer"),
                address,
                Arc::new(Mutex::new(TcpPeerSessionStreamState::new(
                    TcpStream::connect(address).expect("connect heartbeat loopback client"),
                ))),
                Duration::from_secs(60),
            ),
        };
        let (_heartbeat_server, _) = listener.accept().expect("accept heartbeat loopback client");

        managed
            .attach_session(broken_session)
            .expect("attach broken active session");
        let dispatch_error = managed
            .dispatch_transport_command(DaemonTransportCommand::ReleaseAllInputs)
            .expect_err("closed active session dispatch should fail");
        let retry_error = managed
            .connect_session(
                PeerId::new("right-peer"),
                DeviceId::new("windows-desktop"),
                address,
            )
            .expect_err("immediate retry after dispatch failure should be delayed");

        assert!(
            dispatch_error
                .to_string()
                .contains("retry peer session connect after 250ms")
        );
        assert!(
            retry_error
                .to_string()
                .starts_with("peer session reconnect backoff active for ")
        );
        assert_eq!(managed.active_session(), Ok(None));
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
    fn tcp_peer_session_executor_releases_inputs_when_peer_closes_without_release() {
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
            .dispatch_core_actions(&[CoreAction::ForwardInput {
                event: InjectedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            }])
            .expect("TCP peer session dispatch");
        drop(dispatcher);
        drop(transport);

        let (execution, injected_events, release_all_count) = server_thread
            .join()
            .expect("TCP peer session executor thread")
            .expect("TCP peer session execution");

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
    fn tcp_peer_session_executor_releases_inputs_when_heartbeat_timeout_elapses_after_forwarded_input()
     {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread = std::thread::spawn(move || {
            let platform = FakePlatformAdapter::default();
            let error = serve_tcp_peer_transport_session_and_execute_until_closed_with_timeout(
                &listener,
                &platform,
                std::time::Duration::from_millis(50),
            )
            .expect_err("missing heartbeat should fail the peer session");
            let injected_events = platform.injected_events()?;
            let release_all_count = platform.release_all_count()?;

            Ok::<_, PlatformError>((error.to_string(), injected_events, release_all_count))
        });
        let mut stream = std::net::TcpStream::connect(address).expect("connect loopback server");
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );

        super::write_peer_transport_session_frame(&mut stream, &peer_session_hello_frame())
            .expect("write session hello");
        super::write_peer_transport_session_frame(&mut stream, &forward_frame)
            .expect("write forward frame");

        let (error, injected_events, release_all_count) = server_thread
            .join()
            .expect("TCP peer session executor thread")
            .expect("TCP peer timeout execution");

        assert!(
            error.contains("failed to read peer session frame"),
            "unexpected error: {error}"
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
    fn peer_session_executor_releases_inputs_when_stream_errors_after_forwarded_input() {
        let platform = FakePlatformAdapter::default();
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );
        let mut stream = String::new();
        stream.push_str(&peer_session_frame_line(&peer_session_hello_frame()));
        stream.push_str(&peer_session_frame_line(&forward_frame));
        stream.push_str("{\"kind\":\"command\",\"command\":\n");
        let mut reader = std::io::Cursor::new(stream.into_bytes());

        let error = execute_peer_transport_session_stream_until_closed(&mut reader, &platform)
            .expect_err("malformed frame should fail the peer session");

        assert!(
            error
                .to_string()
                .contains("failed to decode peer session frame"),
            "unexpected error: {error}"
        );
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
    fn peer_session_executor_releases_inputs_when_command_payload_is_rejected_after_forwarded_input()
     {
        let platform = FakePlatformAdapter::default();
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );
        let rejected_frame = peer_session_command_frame(
            1,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::Key {
                    key: "notARealKey".to_string(),
                    state: "pressed".to_string(),
                },
            },
        );
        let mut stream = String::new();
        stream.push_str(&peer_session_frame_line(&peer_session_hello_frame()));
        stream.push_str(&peer_session_frame_line(&forward_frame));
        stream.push_str(&peer_session_frame_line(&rejected_frame));
        let mut reader = std::io::Cursor::new(stream.into_bytes());

        let error = execute_peer_transport_session_stream_until_closed(&mut reader, &platform)
            .expect_err("rejected payload should fail the peer session");

        assert_eq!(
            error.to_string(),
            "unsupported peer transport value: notARealKey"
        );
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
    fn peer_session_executor_releases_inputs_when_sequence_mismatch_follows_forwarded_input() {
        let platform = FakePlatformAdapter::default();
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );
        let duplicate_frame =
            peer_session_command_frame(0, PeerTransportCommandPayload::ReleaseAllInputs);
        let mut stream = String::new();
        stream.push_str(&peer_session_frame_line(&peer_session_hello_frame()));
        stream.push_str(&peer_session_frame_line(&forward_frame));
        stream.push_str(&peer_session_frame_line(&duplicate_frame));
        let mut reader = std::io::Cursor::new(stream.into_bytes());

        let error = execute_peer_transport_session_stream_until_closed(&mut reader, &platform)
            .expect_err("duplicate sequence should fail the peer session");

        assert_eq!(
            error.to_string(),
            "peer session sequence mismatch: expected 1, received 0"
        );
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
    fn authenticated_peer_session_executor_applies_commands_after_auth_ready() {
        let platform = FakePlatformAdapter::default();
        let (local, trusted, transcript) = peer_session_auth_fixture();
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );
        let mut stream = String::new();
        stream.push_str(&peer_session_frame_line(&peer_session_hello_frame()));
        stream.push_str(&peer_session_frame_line(&peer_session_auth_proof_frame(
            &local,
            &transcript,
        )));
        stream.push_str(&peer_session_frame_line(&peer_session_ready_frame()));
        stream.push_str(&peer_session_frame_line(&forward_frame));
        let mut reader = std::io::Cursor::new(stream.into_bytes());

        let execution = execute_authenticated_peer_transport_session_stream_until_closed(
            &mut reader,
            &platform,
            &trusted,
            PeerRole::Initiator,
            &transcript,
        )
        .expect("authenticated peer session execution");

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
    fn authenticated_peer_session_executor_rejects_command_before_auth_proof() {
        let platform = FakePlatformAdapter::default();
        let (_, trusted, transcript) = peer_session_auth_fixture();
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );
        let mut stream = String::new();
        stream.push_str(&peer_session_frame_line(&peer_session_hello_frame()));
        stream.push_str(&peer_session_frame_line(&forward_frame));
        let mut reader = std::io::Cursor::new(stream.into_bytes());

        let error = execute_authenticated_peer_transport_session_stream_until_closed(
            &mut reader,
            &platform,
            &trusted,
            PeerRole::Initiator,
            &transcript,
        )
        .expect_err("command before auth proof should fail closed");

        assert_eq!(
            error.to_string(),
            "peer session expected auth proof frame before command frames"
        );
        assert_eq!(
            platform.injected_events().expect("fake injected events"),
            Vec::<InjectedInputEvent>::new()
        );
        assert_eq!(
            platform
                .release_all_count()
                .expect("fake release all count"),
            0
        );
    }

    #[test]
    fn authenticated_peer_session_executor_rejects_invalid_auth_proof_before_input() {
        let platform = FakePlatformAdapter::default();
        let (local, trusted, transcript) = peer_session_auth_fixture();
        let mut proof = local
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .expect("auth proof");
        proof.signature.push(0);
        let forward_frame = peer_session_command_frame(
            0,
            PeerTransportCommandPayload::ForwardInput {
                event: PeerTransportInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            },
        );
        let mut stream = String::new();
        stream.push_str(&peer_session_frame_line(&peer_session_hello_frame()));
        stream.push_str(&peer_session_frame_line(
            &PeerTransportSessionFrame::AuthProof { proof },
        ));
        stream.push_str(&peer_session_frame_line(&peer_session_ready_frame()));
        stream.push_str(&peer_session_frame_line(&forward_frame));
        let mut reader = std::io::Cursor::new(stream.into_bytes());

        let error = execute_authenticated_peer_transport_session_stream_until_closed(
            &mut reader,
            &platform,
            &trusted,
            PeerRole::Initiator,
            &transcript,
        )
        .expect_err("invalid auth proof should fail closed");

        assert!(
            error
                .to_string()
                .starts_with("peer session auth proof rejected for windows-desktop (AKRZ-")
        );
        assert_eq!(
            platform.injected_events().expect("fake injected events"),
            Vec::<InjectedInputEvent>::new()
        );
        assert_eq!(
            platform
                .release_all_count()
                .expect("fake release all count"),
            0
        );
    }

    #[test]
    fn paired_tcp_peer_session_connects_after_auth_and_executes_input() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let (source, trusted_target, trusted_source) = peer_session_pair_auth_fixture();
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept authenticated peer");
            let platform = FakePlatformAdapter::default();
            let execution = execute_paired_tcp_peer_transport_session_until_closed_with_timeout(
                stream,
                &platform,
                std::time::Duration::from_millis(500),
                &DeviceId::new("right-peer"),
                |peer_device_id| {
                    assert_eq!(peer_device_id, &DeviceId::new("windows-desktop"));
                    Ok(trusted_source)
                },
            )?;
            let injected_events = platform.injected_events()?;
            let release_all_count = platform.release_all_count()?;

            Ok::<_, PlatformError>((execution, injected_events, release_all_count))
        });
        let transport = TcpPeerSessionTransport::connect_authenticated(
            PeerId::new("right-peer"),
            &source,
            address,
            &trusted_target,
        )
        .expect("connect authenticated TCP peer session");
        let dispatcher = TransportCoreActionDispatcher::new(transport);

        dispatcher
            .dispatch_core_actions(&[CoreAction::ForwardInput {
                event: InjectedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
            }])
            .expect("dispatch authenticated input");
        drop(dispatcher);

        let (execution, injected_events, release_all_count) = server_thread
            .join()
            .expect("authenticated TCP peer server thread")
            .expect("authenticated TCP peer execution");

        assert_eq!(execution.hello.device_id, DeviceId::new("windows-desktop"));
        assert_eq!(execution.hello.peer_id, PeerId::new("right-peer"));
        assert!(execution.hello.nonce.is_some());
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
        let frame =
            peer_session_command_frame(0, super::PeerTransportCommandPayload::ReleaseAllInputs);
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
    fn daemon_ipc_server_handles_shutdown_request() {
        let platform = FakePlatformAdapter::default();
        let server = DaemonIpcServer::new(RuntimeInputState::new(), platform);
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_DAEMON_SHUTDOWN,
            DaemonShutdownParams::default(),
        );
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line = match server.handle_request_line(&request_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon shutdown response: {error}"),
        };
        let response: JsonRpcSuccess<DaemonShutdownResult> =
            match serde_json::from_str(&response_line) {
                Ok(response) => response,
                Err(error) => panic!("expected daemon shutdown response JSON: {error}"),
            };

        assert!(server.shutdown_requested());
        assert_eq!(response.id, "req_1");
        assert!(response.result.requested);
        assert!(response.result.released_inputs);
        assert!(!response.result.disconnected_peer_session);
        assert_eq!(response.result.mode, ControlModeSnapshot::Local);
    }

    #[test]
    fn ipc_dispatch_rejects_shutdown_without_server_control_flag() {
        let mut state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default();
        let dispatcher =
            LocalPlatformCoreActionDispatcher::new(platform.clone(), NoopCoreActionDispatcher);
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_DAEMON_SHUTDOWN,
            DaemonShutdownParams::default(),
        );
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, &request_line) {
                Ok(line) => line,
                Err(error) => panic!("expected daemon shutdown failure response: {error}"),
            };
        let response: JsonRpcFailure = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected daemon shutdown failure JSON: {error}"),
        };

        assert_eq!(response.id.as_deref(), Some("req_1"));
        assert_eq!(response.error.code, JSONRPC_DAEMON_ERROR);
        assert_eq!(
            response.error.message,
            "daemon shutdown unavailable: shutdown control flag is unavailable"
        );
    }

    #[test]
    fn ipc_dispatch_handles_diagnostics_screen_topology_request() {
        let mut state = RuntimeInputState::new();
        let platform = FakePlatformAdapter::default().with_desktop_geometry(DesktopGeometry {
            pointer_position: LogicalPoint { x: 1919, y: 540 },
            virtual_screen_bounds: LogicalRect {
                origin: LogicalPoint { x: -1920, y: 0 },
                size: LogicalSize {
                    width: 3840,
                    height: 1080,
                },
            },
            monitors: vec![DesktopMonitor {
                id: "primary".to_string(),
                bounds: LogicalRect {
                    origin: LogicalPoint { x: 0, y: 0 },
                    size: LogicalSize {
                        width: 1920,
                        height: 1080,
                    },
                },
                scale_factor_percent: Some(150),
                is_primary: true,
            }],
        });
        let dispatcher =
            LocalPlatformCoreActionDispatcher::new(platform.clone(), NoopCoreActionDispatcher);
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
            DiagnosticsScreenTopologyParams::default(),
        );
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, &request_line) {
                Ok(line) => line,
                Err(error) => panic!("expected daemon IPC response: {error}"),
            };
        let response: JsonRpcSuccess<DiagnosticsScreenTopology> =
            match serde_json::from_str(&response_line) {
                Ok(response) => response,
                Err(error) => panic!("expected screen topology response JSON: {error}"),
            };

        assert_eq!(response.id, "req_1");
        assert_eq!(response.result.pointer_position.x, 1919);
        assert_eq!(response.result.pointer_position.y, 540);
        assert_eq!(response.result.virtual_screen_bounds.x, -1920);
        assert_eq!(response.result.virtual_screen_bounds.y, 0);
        assert_eq!(response.result.virtual_screen_bounds.width, 3840);
        assert_eq!(response.result.virtual_screen_bounds.height, 1080);
        assert_eq!(response.result.monitors.len(), 1);
        assert_eq!(response.result.monitors[0].scale_factor_percent, Some(150));
    }

    #[test]
    fn ipc_dispatch_handles_diagnostics_keyboard_layout_request() {
        let mut state = RuntimeInputState::new();
        let platform =
            FakePlatformAdapter::default().with_keyboard_layout(korean_keyboard_layout());
        let dispatcher =
            LocalPlatformCoreActionDispatcher::new(platform.clone(), NoopCoreActionDispatcher);
        let request = JsonRpcRequest::new(
            "req_1",
            METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT,
            DiagnosticsKeyboardLayoutParams::default(),
        );
        let request_line = match to_json_line(&request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let response_line =
            match handle_ipc_request_line(&mut state, &platform, &dispatcher, &request_line) {
                Ok(line) => line,
                Err(error) => panic!("expected daemon IPC response: {error}"),
            };
        let response: JsonRpcSuccess<DiagnosticsKeyboardLayout> =
            match serde_json::from_str(&response_line) {
                Ok(response) => response,
                Err(error) => panic!("expected keyboard layout response JSON: {error}"),
            };

        assert_eq!(response.id, "req_1");
        assert_eq!(response.result.language_id, "0x0412");
        assert_eq!(response.result.layout_name, Some("00000412".to_string()));
    }

    #[test]
    fn daemon_ipc_server_tails_sanitized_recent_logs() {
        let state = shared_runtime_state(RuntimeInputState::new());
        let platform = FakePlatformAdapter::default();
        let dispatcher = Arc::new(LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            NoopCoreActionDispatcher,
        ));
        let logs = shared_daemon_log_buffer(8);
        let server = DaemonIpcServer::from_shared_state_dispatcher_peer_sessions_and_logs(
            state,
            platform,
            dispatcher,
            ManagedPeerSessionTransport::new(),
            logs,
        );
        let connect_request = JsonRpcRequest::new(
            "req_connect",
            METHOD_SESSION_CONNECT,
            SessionConnectParams {
                peer_id: "secret-peer-id".to_string(),
                local_device_id: "secret-local-device".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
        );
        let connect_line = match to_json_line(&connect_request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };

        let _ = match server.handle_request_line(&connect_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon IPC failure response line: {error}"),
        };

        let logs_request = JsonRpcRequest::new(
            "req_logs",
            METHOD_DAEMON_LOGS_TAIL,
            DaemonLogsTailParams { limit: Some(4) },
        );
        let logs_line = match to_json_line(&logs_request) {
            Ok(line) => line,
            Err(error) => panic!("expected request serialization: {error}"),
        };
        let response_line = match server.handle_request_line(&logs_line) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon logs response: {error}"),
        };
        let response: JsonRpcSuccess<DaemonLogsTail> = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected daemon logs JSON: {error}"),
        };
        let encoded = serde_json::to_string(&response).expect("daemon logs JSON");

        assert_eq!(response.id, "req_logs");
        assert_eq!(response.result.entries.len(), 2);
        assert_eq!(response.result.entries[0].level, DaemonLogLevel::Warn);
        assert_eq!(response.result.entries[0].event, "session.connect.failed");
        assert_eq!(response.result.entries[1].event, "daemon.logs.tail");
        assert!(!encoded.contains("secret-peer-id"));
        assert!(!encoded.contains("secret-local-device"));
        assert!(!encoded.contains("127.0.0.1"));
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
    fn peer_session_connect_command_rejects_unpaired_manager() {
        let peer_sessions = ManagedPeerSessionTransport::new();

        let error = connect_peer_session(
            &SessionConnectParams {
                peer_id: "right-peer".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
            &peer_sessions,
        )
        .expect_err("reject session.connect without identity store");

        assert_eq!(
            error.to_string(),
            "managed peer session requires identity store before session.connect"
        );
        assert_eq!(peer_sessions.active_session(), Ok(None));
    }

    #[test]
    fn peer_session_disconnect_command_recovers_local_control_and_detaches_session() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback TCP listener");
        let address = listener.local_addr().expect("loopback listener address");
        let server_thread =
            std::thread::spawn(move || serve_tcp_peer_transport_session(&listener, 2));
        let mut state = RuntimeInputState::new();
        state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-ipc"),
            })
            .expect("remote entry confirmed");
        let platform = FakePlatformAdapter::default();
        let peer_sessions = ManagedPeerSessionTransport::new();
        let session = TcpPeerSessionTransport::connect(
            PeerId::new("right-peer"),
            DeviceId::new("windows-desktop"),
            address,
        )
        .expect("connect TCP peer session");
        peer_sessions
            .attach_session(session)
            .expect("attach managed peer session");
        let dispatcher = LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            TransportCoreActionDispatcher::new(peer_sessions.clone()),
        );

        let result = disconnect_peer_session(&mut state, &dispatcher, &peer_sessions)
            .expect("disconnect peer session");

        assert_eq!(
            result,
            SessionDisconnectResult {
                disconnected: true,
                session: Some(SessionStatus {
                    peer_id: "right-peer".to_string(),
                    local_device_id: "windows-desktop".to_string(),
                    address: address.to_string(),
                    connected: false,
                }),
                mode: ControlModeSnapshot::Local,
            }
        );
        assert_eq!(state.mode(), ControlMode::Local);
        assert_eq!(platform.release_all_count(), Ok(1));
        assert_eq!(peer_sessions.active_session(), Ok(None));

        let session = server_thread
            .join()
            .expect("TCP peer session server thread")
            .expect("TCP peer session");
        assert_eq!(
            session.commands,
            vec![
                DaemonTransportCommand::ReleaseAllInputs,
                DaemonTransportCommand::StopRemoteSession {
                    session_id: Some(SessionId::new("session-ipc")),
                },
            ]
        );
    }

    #[test]
    fn daemon_ipc_server_rejects_session_connect_without_identity_store() {
        let state = shared_runtime_state(RuntimeInputState::new());
        let platform = FakePlatformAdapter::default();
        let peer_sessions = ManagedPeerSessionTransport::new();
        let dispatcher = LocalPlatformCoreActionDispatcher::new(
            platform.clone(),
            TransportCoreActionDispatcher::new(peer_sessions.clone()),
        );
        let server = DaemonIpcServer::from_shared_state_dispatcher_and_peer_sessions(
            state,
            platform,
            std::sync::Arc::new(dispatcher),
            peer_sessions.clone(),
        );
        let connect_request = JsonRpcRequest::new(
            "req_connect",
            METHOD_SESSION_CONNECT,
            SessionConnectParams {
                peer_id: "right-peer".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
        );
        let connect_line = match to_json_line(&connect_request) {
            Ok(line) => line,
            Err(error) => panic!("expected connect request serialization: {error}"),
        };

        let connect_response_line = match server.handle_request_line(&connect_line) {
            Ok(line) => line,
            Err(error) => panic!("expected connect response: {error}"),
        };
        let connect_response: JsonRpcFailure = match serde_json::from_str(&connect_response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected connect failure JSON: {error}"),
        };

        assert_eq!(connect_response.id, Some("req_connect".to_string()));
        assert_eq!(connect_response.error.code, JSONRPC_DAEMON_ERROR);
        assert_eq!(
            connect_response.error.message,
            "session connect unavailable: managed peer session requires identity store before session.connect"
        );
        assert_eq!(peer_sessions.active_session(), Ok(None));
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
    fn monitored_daemon_input_capture_worker_recovers_after_permission_loss() {
        let platform = FakePlatformAdapter::new(PlatformCapabilities {
            can_capture_pointer: true,
            can_capture_keyboard: false,
            can_inject_pointer: true,
            can_inject_keyboard: true,
        });
        let mut initial_state = RuntimeInputState::new();
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-permission"),
            })
            .expect("remote entry confirmed");
        let state = shared_runtime_state(initial_state);
        let dispatcher = RecordingCoreActionDispatcher::default();
        let logs = shared_daemon_log_buffer(8);
        let expected_actions = vec![
            CoreAction::ReleaseLocalInputs,
            CoreAction::ReleaseAllInputs,
            CoreAction::StopRemoteSession {
                session_id: Some(SessionId::new("session-permission")),
            },
        ];

        let worker = start_monitored_daemon_input_capture_with_edge_bindings_dispatcher_and_logs(
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
            dispatcher.clone(),
            logs.clone(),
        )
        .expect("monitored daemon capture worker");

        for _ in 0..80 {
            if dispatcher.snapshot() == expected_actions {
                worker.stop().expect("stop capture worker");
                assert_eq!(
                    state.lock().expect("shared state lock").mode(),
                    ControlMode::Local
                );
                let entries = logs.lock().expect("daemon logs lock").tail(8);
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].level, DaemonLogLevel::Warn);
                assert_eq!(entries[0].event, "input.capture.permissionLost");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected monitored capture worker to recover after permission loss");
    }

    #[test]
    fn monitored_daemon_input_capture_worker_recovers_after_screen_layout_change() {
        let platform = FakePlatformAdapter::default()
            .with_desktop_geometry(desktop_geometry_at_right_edge())
            .with_open_input_capture();
        let updated_geometry = DesktopGeometry {
            pointer_position: LogicalPoint { x: 2559, y: 720 },
            virtual_screen_bounds: LogicalRect {
                origin: LogicalPoint { x: 0, y: 0 },
                size: LogicalSize {
                    width: 2560,
                    height: 1440,
                },
            },
            monitors: vec![DesktopMonitor {
                id: "primary".to_string(),
                bounds: LogicalRect {
                    origin: LogicalPoint { x: 0, y: 0 },
                    size: LogicalSize {
                        width: 2560,
                        height: 1440,
                    },
                },
                scale_factor_percent: Some(100),
                is_primary: true,
            }],
        };
        let mut initial_state = RuntimeInputState::new();
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("right-peer"),
            })
            .expect("remote entry request");
        initial_state
            .apply_event(RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-layout"),
            })
            .expect("remote entry confirmed");
        let state = shared_runtime_state(initial_state);
        let dispatcher = RecordingCoreActionDispatcher::default();
        let logs = shared_daemon_log_buffer(8);
        let expected_actions = vec![
            CoreAction::ReleaseLocalInputs,
            CoreAction::ReleaseAllInputs,
            CoreAction::StopRemoteSession {
                session_id: Some(SessionId::new("session-layout")),
            },
        ];

        let worker = start_monitored_daemon_input_capture_with_edge_bindings_dispatcher_and_logs(
            state.clone(),
            &platform,
            DaemonInputCaptureConfig {
                input_capture: InputCaptureConfig {
                    event_buffer_capacity: 8,
                },
                drain_batch_size: 8,
                idle_poll_interval: std::time::Duration::from_millis(10),
            },
            vec![ScreenEdgeBinding {
                local_edge: ScreenEdge::Right,
                peer_id: PeerId::new("right-peer"),
                remote_edge: ScreenEdge::Left,
            }],
            dispatcher.clone(),
            logs.clone(),
        )
        .expect("monitored daemon capture worker");

        platform
            .set_desktop_geometry(updated_geometry)
            .expect("update fake desktop geometry");

        for _ in 0..100 {
            let actions = dispatcher.snapshot();
            if actions
                .windows(expected_actions.len())
                .any(|recorded| recorded == expected_actions.as_slice())
            {
                worker.stop().expect("stop capture worker");
                assert_eq!(
                    state.lock().expect("shared state lock").mode(),
                    ControlMode::Local
                );
                let entries = logs.lock().expect("daemon logs lock").tail(8);
                assert!(
                    entries.iter().any(|entry| {
                        entry.level == DaemonLogLevel::Warn
                            && entry.event == "input.capture.layoutChanged"
                    }),
                    "expected layout recovery log entry, got {entries:?}"
                );
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        worker.stop().expect("stop capture worker");
        panic!("expected monitored capture worker to recover after screen layout change");
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
