//! Daemon status builders shared by akraz daemon and diagnostic clients.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use akraz_core::{
    CapturedInputEvent, ControlMode, CoreAction, CoreTransitionError, LogicalPoint, RuntimeEvent,
    RuntimeInputState, ScreenEdgeBinding, ScreenLayout,
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

/// Current daemon package version.
pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

const JSONRPC_DAEMON_ERROR: i32 = -32000;
const DEFAULT_CAPTURE_DRAIN_BATCH_SIZE: usize = 64;
const DEFAULT_CAPTURE_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Shared daemon runtime state observed by IPC and capture workers.
pub type SharedRuntimeInputState = Arc<Mutex<RuntimeInputState>>;

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
    let geometry = platform.read_desktop_geometry()?;

    start_daemon_input_capture_with_routing(
        state,
        platform,
        config,
        DaemonInputRoutingConfig::from_desktop_geometry(geometry, edge_bindings),
    )
}

/// Start daemon input capture with explicit pointer routing configuration.
pub fn start_daemon_input_capture_with_routing(
    state: SharedRuntimeInputState,
    platform: &impl PlatformAdapter,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
) -> Result<DaemonInputCaptureWorker, PlatformError> {
    let capture = platform.start_input_capture(config.input_capture)?;
    let running = Arc::new(AtomicBool::new(true));
    let worker_running = Arc::clone(&running);
    let thread = thread::Builder::new()
        .name("akraz-daemon-input-capture".to_string())
        .spawn(move || {
            run_daemon_input_capture_worker(state, capture, worker_running, config, routing)
        })
        .map_err(|error| {
            PlatformError::new(format!(
                "failed to start daemon input capture worker: {error}"
            ))
        })?;

    Ok(DaemonInputCaptureWorker::new(running, thread))
}

fn run_daemon_input_capture_worker(
    state: SharedRuntimeInputState,
    capture: InputCaptureSession,
    running: Arc<AtomicBool>,
    config: DaemonInputCaptureConfig,
    routing: DaemonInputRoutingConfig,
) -> Result<(), PlatformError> {
    let idle_poll_interval = config.bounded_idle_poll_interval();
    let mut router = CapturedInputRouter::new(routing);

    while running.load(Ordering::Acquire) {
        match capture.recv_timeout(idle_poll_interval) {
            Ok(event) => {
                apply_routed_capture_event(&state, &mut router, event)?;
                drain_ready_capture_events(
                    &state,
                    &capture,
                    &mut router,
                    config.bounded_drain_batch_size(),
                )?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }

    capture.stop()
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
        LogicalPoint, LogicalRect, LogicalSize, PeerId, PhysicalKey, PressState, RuntimeInputState,
        ScreenEdge, ScreenEdgeBinding, ScreenLayout, SessionId,
    };
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, DaemonStatusParams, IpcEndpoint,
        IpcPlatformCapabilities, JsonRpcFailure, JsonRpcRequest, JsonRpcSuccess, LocalIpcServer,
        METHOD_DAEMON_STATUS, OsLocalIpcClient, call_json_rpc, to_json_line,
    };
    use akraz_platform::{
        DesktopGeometry, FakePlatformAdapter, InputCaptureConfig, PlatformAdapter,
        PlatformCapabilities,
    };
    use serde_json::json;

    use super::{
        CapturedInputRouter, DAEMON_VERSION, DaemonInputCaptureConfig, DaemonInputRoutingConfig,
        DaemonIpcRunConfig, DaemonIpcServer, apply_routed_capture_event_to_state,
        build_daemon_status, build_permissions_probe, drain_capture_events,
        handle_ipc_request_line, serve_daemon_ipc, shared_runtime_state,
        start_daemon_input_capture, start_daemon_input_capture_with_edge_bindings,
        start_daemon_input_capture_with_routing,
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
