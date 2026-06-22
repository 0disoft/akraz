//! Platform adapter contract shared by OS-specific akraz crates.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akraz_core::{CapturedInputEvent, InjectedInputEvent, LogicalPoint, LogicalRect, LogicalSize};

#[cfg(all(target_os = "linux", not(test)))]
use std::ffi::{c_char, c_int, c_void};

#[cfg(windows)]
use akraz_core::DEFAULT_PANIC_HOTKEY_KEY;
#[cfg(windows)]
use akraz_core::{MouseButton, PhysicalKey, PressState};

#[cfg(windows)]
use std::collections::BTreeSet;

#[cfg(windows)]
use std::mem::size_of;
#[cfg(windows)]
use std::ptr::{null, null_mut};

#[cfg(windows)]
use std::cell::RefCell;
#[cfg(windows)]
use std::sync::mpsc::TrySendError;
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::thread::JoinHandle;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_HOOKCONTROL, DESKTOP_READOBJECTS, HDESK, OpenInputDesktop,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutNameW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE,
    KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VK_BACK, VK_CONTROL, VK_LCONTROL,
    VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RAWKEYBOARD, RAWMOUSE, RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
    RegisterRawInputDevices,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, HHOOK,
    HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, LLMHF_INJECTED,
    LLMHF_LOWER_IL_INJECTED, MONITORINFOF_PRIMARY, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, RI_KEY_BREAK, RI_KEY_E0, RI_MOUSE_BUTTON_4_DOWN, RI_MOUSE_BUTTON_4_UP,
    RI_MOUSE_BUTTON_5_DOWN, RI_MOUSE_BUTTON_5_UP, RI_MOUSE_HWHEEL, RI_MOUSE_LEFT_BUTTON_DOWN,
    RI_MOUSE_LEFT_BUTTON_UP, RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP,
    RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, RI_MOUSE_WHEEL, RegisterClassW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOWS_HOOK_ID, WM_INPUT,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, XBUTTON1, XBUTTON2,
};
#[cfg(windows)]
use windows_sys::core::BOOL;

/// Capabilities reported by a platform adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformCapabilities {
    pub can_capture_pointer: bool,
    pub can_capture_keyboard: bool,
    pub can_inject_pointer: bool,
    pub can_inject_keyboard: bool,
}

/// Stable diagnostic issue exposed by platform permission probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformDiagnosticIssue {
    pub code: &'static str,
    pub message: &'static str,
}

/// Local desktop geometry facts reported by a platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopGeometry {
    pub pointer_position: LogicalPoint,
    pub virtual_screen_bounds: LogicalRect,
    pub monitors: Vec<DesktopMonitor>,
}

/// Per-monitor geometry facts reported by a platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopMonitor {
    pub id: String,
    pub bounds: LogicalRect,
    pub scale_factor_percent: Option<u32>,
    pub is_primary: bool,
}

/// Sanitized keyboard layout facts reported by a platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardLayoutSnapshot {
    pub source: String,
    pub layout_id: String,
    pub language_id: String,
    pub layout_name: Option<String>,
}

impl PlatformCapabilities {
    /// Return a fully capable adapter profile for deterministic fake platforms.
    pub fn all_enabled() -> Self {
        Self {
            can_capture_pointer: true,
            can_capture_keyboard: true,
            can_inject_pointer: true,
            can_inject_keyboard: true,
        }
    }
}

/// Error returned by a platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError {
    message: String,
}

impl PlatformError {
    /// Create a platform error with a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for PlatformError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PlatformError {}

/// Default number of captured input events buffered before new events are dropped.
pub const DEFAULT_INPUT_CAPTURE_BUFFER_CAPACITY: usize = 256;

/// Configuration used when starting a platform input capture session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCaptureConfig {
    pub event_buffer_capacity: usize,
}

impl InputCaptureConfig {
    fn bounded_capacity(self) -> usize {
        self.event_buffer_capacity.max(1)
    }
}

impl Default for InputCaptureConfig {
    fn default() -> Self {
        Self {
            event_buffer_capacity: DEFAULT_INPUT_CAPTURE_BUFFER_CAPACITY,
        }
    }
}

/// Local handling policy for captured OS input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputCapturePolicy {
    /// Observe input and let the local desktop continue processing it.
    #[default]
    PassThrough,
    /// Forward captured input and prevent the local desktop from also processing it.
    ConsumeCapturedInput,
}

impl InputCapturePolicy {
    fn as_u8(self) -> u8 {
        match self {
            Self::PassThrough => 0,
            Self::ConsumeCapturedInput => 1,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::ConsumeCapturedInput,
            _ => Self::PassThrough,
        }
    }

    #[cfg(windows)]
    fn consumes_captured_input(self) -> bool {
        self == Self::ConsumeCapturedInput
    }
}

#[derive(Debug, Clone)]
struct SharedInputCapturePolicy {
    value: Arc<AtomicU8>,
}

impl SharedInputCapturePolicy {
    fn new(policy: InputCapturePolicy) -> Self {
        Self {
            value: Arc::new(AtomicU8::new(policy.as_u8())),
        }
    }

    fn load(&self) -> InputCapturePolicy {
        InputCapturePolicy::from_u8(self.value.load(Ordering::Acquire))
    }

    fn store(&self, policy: InputCapturePolicy) {
        self.value.store(policy.as_u8(), Ordering::Release);
    }
}

impl Default for SharedInputCapturePolicy {
    fn default() -> Self {
        Self::new(InputCapturePolicy::default())
    }
}

/// Running platform input capture session.
pub struct InputCaptureSession {
    events: Receiver<CapturedInputEvent>,
    shutdown: Option<InputCaptureShutdown>,
    policy: SharedInputCapturePolicy,
}

impl InputCaptureSession {
    fn new(
        events: Receiver<CapturedInputEvent>,
        shutdown: InputCaptureShutdown,
        policy: SharedInputCapturePolicy,
    ) -> Self {
        Self {
            events,
            shutdown: Some(shutdown),
            policy,
        }
    }

    /// Return the local input handling policy used by this capture session.
    pub fn policy(&self) -> InputCapturePolicy {
        self.policy.load()
    }

    /// Update whether captured input should pass through to the local desktop or be consumed.
    pub fn set_policy(&self, policy: InputCapturePolicy) {
        self.policy.store(policy);
    }

    /// Receive the next captured input event without blocking.
    pub fn try_recv(&self) -> Result<CapturedInputEvent, TryRecvError> {
        self.events.try_recv()
    }

    /// Receive the next captured input event until the timeout expires.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<CapturedInputEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Stop input capture and wait for its background resources to exit.
    pub fn stop(mut self) -> Result<(), PlatformError> {
        stop_input_capture(self.shutdown.take())
    }
}

impl Drop for InputCaptureSession {
    fn drop(&mut self) {
        let _ = stop_input_capture(self.shutdown.take());
    }
}

enum InputCaptureShutdown {
    Noop,
    #[cfg(windows)]
    Windows(WindowsCaptureShutdown),
}

impl InputCaptureShutdown {
    fn stop(self) -> Result<(), PlatformError> {
        match self {
            Self::Noop => Ok(()),
            #[cfg(windows)]
            Self::Windows(shutdown) => shutdown.stop(),
        }
    }
}

fn stop_input_capture(shutdown: Option<InputCaptureShutdown>) -> Result<(), PlatformError> {
    match shutdown {
        Some(shutdown) => shutdown.stop(),
        None => Ok(()),
    }
}

/// OS-independent platform adapter interface.
pub trait PlatformAdapter {
    /// Stable adapter name, such as `windows` or `fake`.
    fn name(&self) -> &'static str;

    /// Probe platform input capabilities.
    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError>;

    /// Return platform-specific permission diagnostics that explain unavailable capabilities.
    fn diagnostic_permission_issues(&self) -> Vec<PlatformDiagnosticIssue> {
        Vec::new()
    }

    /// Read current desktop geometry used by input routing.
    fn read_desktop_geometry(&self) -> Result<DesktopGeometry, PlatformError> {
        Err(PlatformError::new(format!(
            "{} desktop geometry is not available",
            self.name()
        )))
    }

    /// Read sanitized keyboard layout facts used for IME diagnostics.
    fn read_keyboard_layout(&self) -> Result<KeyboardLayoutSnapshot, PlatformError> {
        Err(PlatformError::new(format!(
            "{} keyboard layout is not available",
            self.name()
        )))
    }

    /// Start capturing platform input events into a bounded event queue.
    fn start_input_capture(
        &self,
        _config: InputCaptureConfig,
    ) -> Result<InputCaptureSession, PlatformError> {
        Err(PlatformError::new(format!(
            "{} input capture is not available",
            self.name()
        )))
    }

    /// Inject one normalized input event into the local desktop.
    fn inject_input(&self, _event: &InjectedInputEvent) -> Result<(), PlatformError> {
        Err(PlatformError::new(format!(
            "{} input injection is not available",
            self.name()
        )))
    }

    /// Release all currently pressed keys and buttons known to the adapter.
    fn release_all(&self) -> Result<(), PlatformError>;
}

/// Runtime platform adapter selected for the current operating system.
#[derive(Debug, Clone)]
pub enum RuntimePlatformAdapter {
    #[cfg(windows)]
    Windows(WindowsPlatformAdapter),
    #[cfg(not(windows))]
    Unsupported(UnsupportedPlatformAdapter),
}

impl PlatformAdapter for RuntimePlatformAdapter {
    fn name(&self) -> &'static str {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.name(),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.name(),
        }
    }

    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.probe_capabilities(),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.probe_capabilities(),
        }
    }

    fn diagnostic_permission_issues(&self) -> Vec<PlatformDiagnosticIssue> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.diagnostic_permission_issues(),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.diagnostic_permission_issues(),
        }
    }

    fn read_desktop_geometry(&self) -> Result<DesktopGeometry, PlatformError> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.read_desktop_geometry(),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.read_desktop_geometry(),
        }
    }

    fn read_keyboard_layout(&self) -> Result<KeyboardLayoutSnapshot, PlatformError> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.read_keyboard_layout(),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.read_keyboard_layout(),
        }
    }

    fn start_input_capture(
        &self,
        config: InputCaptureConfig,
    ) -> Result<InputCaptureSession, PlatformError> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.start_input_capture(config),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.start_input_capture(config),
        }
    }

    fn inject_input(&self, event: &InjectedInputEvent) -> Result<(), PlatformError> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.inject_input(event),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.inject_input(event),
        }
    }

    fn release_all(&self) -> Result<(), PlatformError> {
        match self {
            #[cfg(windows)]
            Self::Windows(adapter) => adapter.release_all(),
            #[cfg(not(windows))]
            Self::Unsupported(adapter) => adapter.release_all(),
        }
    }
}

/// Select the platform adapter used by production daemon runtime paths.
pub fn runtime_platform_adapter() -> RuntimePlatformAdapter {
    #[cfg(windows)]
    {
        RuntimePlatformAdapter::Windows(WindowsPlatformAdapter::new())
    }

    #[cfg(not(windows))]
    {
        RuntimePlatformAdapter::Unsupported(UnsupportedPlatformAdapter::new())
    }
}

/// Windows platform adapter.
#[cfg(windows)]
#[derive(Debug, Clone, Default)]
pub struct WindowsPlatformAdapter {
    injected_state: Arc<Mutex<WindowsInjectedInputState>>,
}

#[cfg(windows)]
impl WindowsPlatformAdapter {
    /// Create the Windows platform adapter.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(windows)]
impl PlatformAdapter for WindowsPlatformAdapter {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        Ok(probe_windows_capabilities())
    }

    fn read_desktop_geometry(&self) -> Result<DesktopGeometry, PlatformError> {
        read_windows_desktop_geometry()
    }

    fn read_keyboard_layout(&self) -> Result<KeyboardLayoutSnapshot, PlatformError> {
        read_windows_keyboard_layout()
    }

    fn start_input_capture(
        &self,
        config: InputCaptureConfig,
    ) -> Result<InputCaptureSession, PlatformError> {
        start_windows_input_capture(config)
    }

    fn inject_input(&self, event: &InjectedInputEvent) -> Result<(), PlatformError> {
        inject_windows_input(&self.injected_state, event)
    }

    fn release_all(&self) -> Result<(), PlatformError> {
        release_all_windows_inputs(&self.injected_state)
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsProbeResults {
    can_open_input_desktop: bool,
    can_install_mouse_hook: bool,
    can_install_keyboard_hook: bool,
    can_send_mouse_input: bool,
}

#[cfg(windows)]
fn probe_windows_capabilities() -> PlatformCapabilities {
    let can_open_input_desktop = InputDesktopHandle::open_for_hook_probe().is_some();
    let can_install_mouse_hook =
        can_open_input_desktop && LowLevelHookHandle::install(WH_MOUSE_LL).is_some();
    let can_install_keyboard_hook =
        can_open_input_desktop && LowLevelHookHandle::install(WH_KEYBOARD_LL).is_some();

    windows_capabilities_from_probe_results(WindowsProbeResults {
        can_open_input_desktop,
        can_install_mouse_hook,
        can_install_keyboard_hook,
        can_send_mouse_input: can_send_zero_delta_mouse_input(),
    })
}

#[cfg(windows)]
fn windows_capabilities_from_probe_results(results: WindowsProbeResults) -> PlatformCapabilities {
    PlatformCapabilities {
        can_capture_pointer: results.can_open_input_desktop && results.can_install_mouse_hook,
        can_capture_keyboard: results.can_open_input_desktop && results.can_install_keyboard_hook,
        can_inject_pointer: results.can_send_mouse_input,
        can_inject_keyboard: results.can_send_mouse_input,
    }
}

#[cfg(windows)]
fn read_windows_desktop_geometry() -> Result<DesktopGeometry, PlatformError> {
    let pointer_position = read_windows_pointer_position()?;
    let virtual_screen_bounds = read_windows_virtual_screen_bounds()?;
    let monitors = read_windows_monitor_snapshots();

    Ok(DesktopGeometry {
        pointer_position,
        virtual_screen_bounds,
        monitors,
    })
}

#[cfg(windows)]
fn read_windows_pointer_position() -> Result<LogicalPoint, PlatformError> {
    let mut point = POINT::default();
    // SAFETY: point is a valid POINT pointer that Windows writes synchronously.
    let ok = unsafe { GetCursorPos(&mut point) };
    if ok == 0 {
        return Err(PlatformError::new("failed to read cursor position"));
    }

    Ok(LogicalPoint {
        x: point.x,
        y: point.y,
    })
}

#[cfg(windows)]
fn read_windows_virtual_screen_bounds() -> Result<LogicalRect, PlatformError> {
    let bounds = LogicalRect {
        origin: LogicalPoint {
            // SAFETY: GetSystemMetrics does not dereference Rust pointers.
            x: unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
            // SAFETY: GetSystemMetrics does not dereference Rust pointers.
            y: unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
        },
        size: LogicalSize {
            // SAFETY: GetSystemMetrics does not dereference Rust pointers.
            width: unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
            // SAFETY: GetSystemMetrics does not dereference Rust pointers.
            height: unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
        },
    };

    if !bounds.is_valid() {
        return Err(PlatformError::new("failed to read virtual screen bounds"));
    }

    Ok(bounds)
}

#[cfg(windows)]
fn read_windows_monitor_snapshots() -> Vec<DesktopMonitor> {
    let mut monitors = Vec::new();
    let monitors_ptr = &mut monitors as *mut Vec<DesktopMonitor>;
    let ok = unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(enum_windows_monitor_snapshot),
            monitors_ptr as LPARAM,
        )
    };

    if ok == 0 { Vec::new() } else { monitors }
}

#[cfg(windows)]
unsafe extern "system" fn enum_windows_monitor_snapshot(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    monitors_ptr: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(monitors_ptr as *mut Vec<DesktopMonitor>) };
    if let Some(snapshot) = read_windows_monitor_snapshot(monitor, monitors.len()) {
        monitors.push(snapshot);
    }

    1
}

#[cfg(windows)]
fn read_windows_keyboard_layout() -> Result<KeyboardLayoutSnapshot, PlatformError> {
    let (thread_id, source) = foreground_window_thread_id()
        .map(|thread_id| (thread_id, "foregroundWindowThread"))
        .unwrap_or_else(|| {
            let thread_id = unsafe { GetCurrentThreadId() };
            (thread_id, "currentThread")
        });

    let layout = unsafe { GetKeyboardLayout(thread_id) };
    if layout.is_null() {
        return Err(PlatformError::new("failed to read keyboard layout"));
    }

    let layout_id = layout as usize;
    let language_id = layout_id & 0xffff;

    Ok(KeyboardLayoutSnapshot {
        source: source.to_string(),
        layout_id: format!("0x{layout_id:016X}"),
        language_id: format!("0x{language_id:04X}"),
        layout_name: read_windows_keyboard_layout_name(),
    })
}

#[cfg(windows)]
fn foreground_window_thread_id() -> Option<u32> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }

    let thread_id = unsafe { GetWindowThreadProcessId(foreground, null_mut()) };
    if thread_id == 0 {
        None
    } else {
        Some(thread_id)
    }
}

#[cfg(windows)]
fn read_windows_keyboard_layout_name() -> Option<String> {
    let mut buffer = [0_u16; 9];
    let ok = unsafe { GetKeyboardLayoutNameW(buffer.as_mut_ptr()) };
    if ok == 0 {
        return None;
    }

    let end = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

#[cfg(windows)]
fn read_windows_monitor_snapshot(
    monitor: HMONITOR,
    fallback_index: usize,
) -> Option<DesktopMonitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;

    let ok = unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) };
    if ok == 0 {
        return None;
    }

    let bounds = rect_to_logical_rect(info.monitorInfo.rcMonitor);
    if !bounds.is_valid() {
        return None;
    }

    Some(DesktopMonitor {
        id: monitor_device_id(&info).unwrap_or_else(|| format!("monitor-{}", fallback_index + 1)),
        bounds,
        scale_factor_percent: read_monitor_scale_factor_percent(monitor),
        is_primary: (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
    })
}

#[cfg(windows)]
fn rect_to_logical_rect(rect: RECT) -> LogicalRect {
    LogicalRect {
        origin: LogicalPoint {
            x: rect.left,
            y: rect.top,
        },
        size: LogicalSize {
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        },
    }
}

#[cfg(windows)]
fn monitor_device_id(info: &MONITORINFOEXW) -> Option<String> {
    let end = info
        .szDevice
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(info.szDevice.len());
    if end == 0 {
        return None;
    }

    Some(String::from_utf16_lossy(&info.szDevice[..end]))
}

#[cfg(windows)]
fn read_monitor_scale_factor_percent(monitor: HMONITOR) -> Option<u32> {
    let mut dpi_x = 0_u32;
    let mut dpi_y = 0_u32;
    let result = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    if result < 0 || dpi_x == 0 || dpi_y == 0 {
        return None;
    }

    Some(((u64::from(dpi_x) * 100 + 48) / 96) as u32)
}

#[cfg(windows)]
thread_local! {
    static WINDOWS_CAPTURE_STATE: RefCell<WindowsCaptureThreadState> =
        RefCell::new(WindowsCaptureThreadState::default());
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsCaptureThreadState {
    sender: Option<SyncSender<CapturedInputEvent>>,
    policy: Option<SharedInputCapturePolicy>,
    raw_input_enabled: bool,
    last_pointer: Option<(i32, i32)>,
    last_raw_absolute_pointer: Option<(i32, i32)>,
}

#[cfg(windows)]
fn start_windows_input_capture(
    config: InputCaptureConfig,
) -> Result<InputCaptureSession, PlatformError> {
    let (event_sender, event_receiver) = sync_channel(config.bounded_capacity());
    let (ready_sender, ready_receiver) = sync_channel(1);
    let policy = SharedInputCapturePolicy::default();
    let thread_policy = policy.clone();
    let thread = thread::Builder::new()
        .name("akraz-windows-input-capture".to_string())
        .spawn(move || run_windows_input_capture_thread(event_sender, ready_sender, thread_policy))
        .map_err(|error| PlatformError::new(format!("failed to start input capture: {error}")))?;

    match ready_receiver.recv() {
        Ok(Ok(thread_id)) => Ok(InputCaptureSession::new(
            event_receiver,
            InputCaptureShutdown::Windows(WindowsCaptureShutdown {
                thread_id,
                thread: Some(thread),
            }),
            policy,
        )),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(error) => {
            let _ = thread.join();
            Err(PlatformError::new(format!(
                "input capture failed before startup: {error}"
            )))
        }
    }
}

#[cfg(windows)]
fn run_windows_input_capture_thread(
    sender: SyncSender<CapturedInputEvent>,
    ready: SyncSender<Result<u32, PlatformError>>,
    policy: SharedInputCapturePolicy,
) {
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut message = MSG::default();
    // SAFETY: message is a valid MSG pointer and a null HWND creates this thread's message queue.
    unsafe {
        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
    }

    WINDOWS_CAPTURE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.sender = Some(sender);
        state.policy = Some(policy);
        state.raw_input_enabled = false;
        state.last_pointer = None;
        state.last_raw_absolute_pointer = None;
    });

    let result = run_windows_input_capture_message_loop(thread_id, &ready);

    WINDOWS_CAPTURE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.sender = None;
        state.policy = None;
        state.raw_input_enabled = false;
        state.last_pointer = None;
        state.last_raw_absolute_pointer = None;
    });

    if let Err(error) = result {
        let _ = ready.try_send(Err(error));
    }
}

#[cfg(windows)]
fn run_windows_input_capture_message_loop(
    thread_id: u32,
    ready: &SyncSender<Result<u32, PlatformError>>,
) -> Result<(), PlatformError> {
    let _input_desktop = InputDesktopHandle::open_for_hook_probe()
        .ok_or_else(|| PlatformError::new("failed to open input desktop for capture"))?;
    let _hidden_window = WindowsHiddenMessageWindow::create()?;
    register_windows_raw_input(_hidden_window.hwnd())?;
    WINDOWS_CAPTURE_STATE.with(|state| {
        state.borrow_mut().raw_input_enabled = true;
    });

    let _mouse_hook = LowLevelHookHandle::install(WH_MOUSE_LL)
        .ok_or_else(|| PlatformError::new("failed to install low-level mouse hook"))?;
    let _keyboard_hook = LowLevelHookHandle::install(WH_KEYBOARD_LL)
        .ok_or_else(|| PlatformError::new("failed to install low-level keyboard hook"))?;

    let _ = ready.try_send(Ok(thread_id));

    let mut message = MSG::default();
    loop {
        // SAFETY: message is a valid MSG pointer and a null HWND retrieves thread messages.
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result == -1 {
            return Err(PlatformError::new("input capture message loop failed"));
        }
        if result == 0 || message.message == WM_QUIT {
            return Ok(());
        }
        // SAFETY: message was returned by GetMessageW and is dispatched on the same thread.
        unsafe {
            DispatchMessageW(&message);
        }
    }
}

#[cfg(windows)]
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
#[cfg(windows)]
const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
#[cfg(windows)]
const HID_USAGE_GENERIC_KEYBOARD: u16 = 0x06;

#[cfg(windows)]
#[derive(Debug)]
struct WindowsHiddenMessageWindow {
    hwnd: HWND,
}

#[cfg(windows)]
impl WindowsHiddenMessageWindow {
    fn create() -> Result<Self, PlatformError> {
        let class_name = windows_capture_window_class_name();
        // SAFETY: A null module name asks Windows for the module handle of this process image.
        let module = unsafe { GetModuleHandleW(null()) };
        if module.is_null() {
            return Err(PlatformError::new(
                "failed to load module handle for raw input window",
            ));
        }

        let window_class = WNDCLASSW {
            lpfnWndProc: Some(windows_raw_input_window_proc),
            hInstance: module,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        // SAFETY: window_class contains a stable class-name pointer for the duration of this call.
        unsafe {
            RegisterClassW(&window_class);
        }

        // SAFETY: class_name is a null-terminated UTF-16 string. HWND_MESSAGE creates a
        // message-only hidden window on this thread. The HWND is owned by WindowsHiddenMessageWindow.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                module,
                null(),
            )
        };
        if hwnd.is_null() {
            Err(PlatformError::new(
                "failed to create hidden raw input message window",
            ))
        } else {
            Ok(Self { hwnd })
        }
    }

    fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

#[cfg(windows)]
impl Drop for WindowsHiddenMessageWindow {
    fn drop(&mut self) {
        // SAFETY: self.hwnd is a non-null HWND created by CreateWindowExW and owned by this wrapper.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(windows)]
fn windows_capture_window_class_name() -> Vec<u16> {
    "AkrazRawInputCaptureWindow\0".encode_utf16().collect()
}

#[cfg(windows)]
fn register_windows_raw_input(hwnd: HWND) -> Result<(), PlatformError> {
    let devices = windows_raw_input_devices(hwnd);
    // SAFETY: devices points to two initialized RAWINPUTDEVICE records and cbSize matches the ABI.
    let registered = unsafe {
        RegisterRawInputDevices(
            devices.as_ptr(),
            devices.len() as u32,
            size_of::<RAWINPUTDEVICE>() as u32,
        )
    };

    if registered == 0 {
        Err(PlatformError::new(
            "failed to register Windows raw input devices",
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn windows_raw_input_devices(hwnd: HWND) -> [RAWINPUTDEVICE; 2] {
    [
        RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: HID_USAGE_GENERIC_MOUSE,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: HID_USAGE_GENERIC_KEYBOARD,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
    ]
}

#[cfg(windows)]
unsafe extern "system" fn windows_raw_input_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_INPUT {
        forward_captured_raw_input(lparam);
        // SAFETY: WM_INPUT still flows through DefWindowProcW after Akraz consumes the
        // raw payload so Windows can complete its normal message cleanup path.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    // SAFETY: Unhandled messages are delegated to the system default window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(windows)]
struct WindowsCaptureShutdown {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

#[cfg(windows)]
impl WindowsCaptureShutdown {
    fn stop(mut self) -> Result<(), PlatformError> {
        // SAFETY: thread_id is the id reported by the capture thread after creating its message
        // queue. Posting WM_QUIT asks the capture thread to leave GetMessageW.
        let posted = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if posted == 0 {
            return Err(PlatformError::new("failed to stop input capture thread"));
        }

        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| PlatformError::new("input capture thread panicked"))?;
        }

        Ok(())
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct InputDesktopHandle {
    handle: HDESK,
}

#[cfg(windows)]
impl InputDesktopHandle {
    fn open_for_hook_probe() -> Option<Self> {
        let desired_access = DESKTOP_READOBJECTS | DESKTOP_HOOKCONTROL;
        // SAFETY: The call does not dereference Rust pointers. A non-null desktop handle is
        // wrapped immediately and closed by Drop.
        let handle = unsafe { OpenInputDesktop(0, 0, desired_access) };

        if handle.is_null() {
            None
        } else {
            Some(Self { handle })
        }
    }
}

#[cfg(windows)]
impl Drop for InputDesktopHandle {
    fn drop(&mut self) {
        // SAFETY: self.handle is a non-null HDESK returned by OpenInputDesktop and is owned by
        // this wrapper. Drop cannot recover from cleanup failure.
        unsafe {
            CloseDesktop(self.handle);
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct LowLevelHookHandle {
    handle: HHOOK,
}

#[cfg(windows)]
impl LowLevelHookHandle {
    fn install(hook_id: WINDOWS_HOOK_ID) -> Option<Self> {
        // SAFETY: A null module name asks Windows for the module handle of this process image.
        let module = unsafe { GetModuleHandleW(null()) };
        if module.is_null() {
            return None;
        }

        // SAFETY: low_level_hook_proc has the required system ABI and does not capture Rust
        // state. The returned hook handle is owned by LowLevelHookHandle and unhooked in Drop.
        let handle = unsafe { SetWindowsHookExW(hook_id, Some(low_level_hook_proc), module, 0) };
        if handle.is_null() {
            None
        } else {
            Some(Self { handle })
        }
    }
}

#[cfg(windows)]
impl Drop for LowLevelHookHandle {
    fn drop(&mut self) {
        // SAFETY: self.handle is a non-null HHOOK returned by SetWindowsHookExW and is owned by
        // this wrapper. Drop cannot recover from cleanup failure.
        unsafe {
            UnhookWindowsHookEx(self.handle);
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn low_level_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mut consume = false;
    if code >= 0 {
        consume = forward_captured_windows_event(wparam, lparam);
    }

    if consume {
        return 1;
    }

    // SAFETY: The hook forwards notifications that are not captured under the active policy.
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

#[cfg(windows)]
fn forward_captured_windows_event(wparam: WPARAM, lparam: LPARAM) -> bool {
    WINDOWS_CAPTURE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(capture) = windows_capture_from_hook(wparam as u32, lparam, &mut state) else {
            return false;
        };
        if !state.raw_input_enabled
            && let Some(sender) = state.sender.as_ref()
        {
            try_send_captured_input_event(sender, capture.event);
        }

        capture.consume
    })
}

#[cfg(windows)]
fn try_send_captured_input_event(
    sender: &SyncSender<CapturedInputEvent>,
    event: CapturedInputEvent,
) {
    match sender.try_send(event) {
        Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
}

#[cfg(windows)]
fn forward_captured_raw_input(lparam: LPARAM) {
    WINDOWS_CAPTURE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(raw_input) = windows_raw_input_from_lparam(lparam) else {
            return;
        };
        let events = captured_events_from_windows_raw_input(raw_input, &mut state);
        let Some(sender) = state.sender.as_ref() else {
            return;
        };

        for event in events {
            try_send_captured_input_event(sender, event);
        }
    });
}

#[cfg(windows)]
fn windows_raw_input_from_lparam(lparam: LPARAM) -> Option<RAWINPUT> {
    if lparam == 0 {
        return None;
    }

    let handle = lparam as HRAWINPUT;
    let mut raw_input = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: raw_input points to an initialized RAWINPUT buffer and size points to its byte length.
    let read = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            &mut raw_input as *mut RAWINPUT as *mut _,
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };

    if read == u32::MAX || read == 0 {
        None
    } else {
        Some(raw_input)
    }
}

#[cfg(windows)]
fn captured_events_from_windows_raw_input(
    raw_input: RAWINPUT,
    state: &mut WindowsCaptureThreadState,
) -> Vec<CapturedInputEvent> {
    match raw_input.header.dwType {
        RIM_TYPEMOUSE => {
            // SAFETY: dwType identifies the active RAWINPUT union member as mouse.
            let mouse = unsafe { raw_input.data.mouse };
            captured_events_from_windows_raw_mouse(mouse, state)
        }
        RIM_TYPEKEYBOARD => {
            // SAFETY: dwType identifies the active RAWINPUT union member as keyboard.
            let keyboard = unsafe { raw_input.data.keyboard };
            captured_event_from_windows_raw_keyboard(keyboard)
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(windows)]
fn captured_events_from_windows_raw_mouse(
    mouse: RAWMOUSE,
    state: &mut WindowsCaptureThreadState,
) -> Vec<CapturedInputEvent> {
    let mut events = Vec::new();
    if let Some(event) = captured_pointer_move_from_raw_mouse(mouse, state) {
        events.push(event);
    }

    let (button_flags, button_data) = raw_mouse_button_fields(mouse);
    push_raw_mouse_button_event(
        &mut events,
        button_flags,
        RI_MOUSE_LEFT_BUTTON_DOWN,
        RI_MOUSE_LEFT_BUTTON_UP,
        MouseButton::Left,
    );
    push_raw_mouse_button_event(
        &mut events,
        button_flags,
        RI_MOUSE_RIGHT_BUTTON_DOWN,
        RI_MOUSE_RIGHT_BUTTON_UP,
        MouseButton::Right,
    );
    push_raw_mouse_button_event(
        &mut events,
        button_flags,
        RI_MOUSE_MIDDLE_BUTTON_DOWN,
        RI_MOUSE_MIDDLE_BUTTON_UP,
        MouseButton::Middle,
    );
    push_raw_mouse_button_event(
        &mut events,
        button_flags,
        RI_MOUSE_BUTTON_4_DOWN,
        RI_MOUSE_BUTTON_4_UP,
        MouseButton::Back,
    );
    push_raw_mouse_button_event(
        &mut events,
        button_flags,
        RI_MOUSE_BUTTON_5_DOWN,
        RI_MOUSE_BUTTON_5_UP,
        MouseButton::Forward,
    );

    let wheel_delta = raw_mouse_button_data_delta(button_data);
    if button_flags & RI_MOUSE_WHEEL as u16 != 0 && wheel_delta != 0 {
        events.push(CapturedInputEvent::Scroll {
            delta_x: 0,
            delta_y: wheel_delta,
        });
    }
    if button_flags & RI_MOUSE_HWHEEL as u16 != 0 && wheel_delta != 0 {
        events.push(CapturedInputEvent::Scroll {
            delta_x: wheel_delta,
            delta_y: 0,
        });
    }

    events
}

#[cfg(windows)]
fn captured_pointer_move_from_raw_mouse(
    mouse: RAWMOUSE,
    state: &mut WindowsCaptureThreadState,
) -> Option<CapturedInputEvent> {
    let (delta_x, delta_y) = if mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
        let next = (mouse.lLastX, mouse.lLastY);
        let previous = state.last_raw_absolute_pointer.replace(next)?;
        (
            next.0.saturating_sub(previous.0),
            next.1.saturating_sub(previous.1),
        )
    } else {
        state.last_raw_absolute_pointer = None;
        (mouse.lLastX, mouse.lLastY)
    };

    if delta_x == 0 && delta_y == 0 {
        None
    } else {
        Some(CapturedInputEvent::PointerMoved { delta_x, delta_y })
    }
}

#[cfg(windows)]
fn raw_mouse_button_fields(mouse: RAWMOUSE) -> (u16, u16) {
    // SAFETY: Accessing the documented button-fields view of RAWMOUSE's anonymous union.
    let fields = unsafe { mouse.Anonymous.Anonymous };
    (fields.usButtonFlags, fields.usButtonData)
}

#[cfg(windows)]
fn push_raw_mouse_button_event(
    events: &mut Vec<CapturedInputEvent>,
    button_flags: u16,
    down_flag: u32,
    up_flag: u32,
    button: MouseButton,
) {
    if button_flags & down_flag as u16 != 0 {
        events.push(CapturedInputEvent::MouseButton {
            button,
            state: PressState::Pressed,
        });
    }
    if button_flags & up_flag as u16 != 0 {
        events.push(CapturedInputEvent::MouseButton {
            button,
            state: PressState::Released,
        });
    }
}

#[cfg(windows)]
fn raw_mouse_button_data_delta(button_data: u16) -> i32 {
    (button_data as i16) as i32
}

#[cfg(windows)]
fn captured_event_from_windows_raw_keyboard(keyboard: RAWKEYBOARD) -> Option<CapturedInputEvent> {
    if keyboard.MakeCode == 0 && keyboard.VKey == 0 {
        return None;
    }

    let state = if keyboard.Flags & RI_KEY_BREAK as u16 != 0 {
        PressState::Released
    } else {
        PressState::Pressed
    };

    Some(CapturedInputEvent::Key {
        key: physical_key_from_windows_raw_keyboard(keyboard),
        state,
    })
}

#[cfg(windows)]
fn physical_key_from_windows_raw_keyboard(keyboard: RAWKEYBOARD) -> PhysicalKey {
    let extended = keyboard.Flags & RI_KEY_E0 as u16 != 0;
    match keyboard.VKey {
        VK_SHIFT => match keyboard.MakeCode {
            0x36 => PhysicalKey::RightShift,
            _ => PhysicalKey::LeftShift,
        },
        VK_CONTROL => {
            if extended {
                PhysicalKey::RightControl
            } else {
                PhysicalKey::LeftControl
            }
        }
        VK_MENU => {
            if extended {
                PhysicalKey::RightAlt
            } else {
                PhysicalKey::LeftAlt
            }
        }
        VK_LSHIFT => PhysicalKey::LeftShift,
        VK_RSHIFT => PhysicalKey::RightShift,
        VK_LCONTROL => PhysicalKey::LeftControl,
        VK_RCONTROL => PhysicalKey::RightControl,
        VK_LMENU => PhysicalKey::LeftAlt,
        VK_RMENU => PhysicalKey::RightAlt,
        VK_LWIN => PhysicalKey::LeftMeta,
        VK_RWIN => PhysicalKey::RightMeta,
        VK_BACK => DEFAULT_PANIC_HOTKEY_KEY,
        _ => PhysicalKey::Code(keyboard.MakeCode),
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsCapturedHookEvent {
    event: CapturedInputEvent,
    consume: bool,
}

#[cfg(windows)]
fn windows_capture_from_hook(
    message: u32,
    lparam: LPARAM,
    state: &mut WindowsCaptureThreadState,
) -> Option<WindowsCapturedHookEvent> {
    let event = captured_event_from_windows_hook(message, lparam, state)?;
    let consume = state
        .policy
        .as_ref()
        .is_some_and(|policy| policy.load().consumes_captured_input());

    Some(WindowsCapturedHookEvent { event, consume })
}

#[cfg(windows)]
fn captured_event_from_windows_hook(
    message: u32,
    lparam: LPARAM,
    state: &mut WindowsCaptureThreadState,
) -> Option<CapturedInputEvent> {
    match message {
        WM_MOUSEMOVE => captured_pointer_move(lparam, state),
        WM_LBUTTONDOWN => captured_mouse_button(lparam, MouseButton::Left, PressState::Pressed),
        WM_LBUTTONUP => captured_mouse_button(lparam, MouseButton::Left, PressState::Released),
        WM_RBUTTONDOWN => captured_mouse_button(lparam, MouseButton::Right, PressState::Pressed),
        WM_RBUTTONUP => captured_mouse_button(lparam, MouseButton::Right, PressState::Released),
        WM_MBUTTONDOWN => captured_mouse_button(lparam, MouseButton::Middle, PressState::Pressed),
        WM_MBUTTONUP => captured_mouse_button(lparam, MouseButton::Middle, PressState::Released),
        WM_XBUTTONDOWN => captured_x_button(lparam, PressState::Pressed),
        WM_XBUTTONUP => captured_x_button(lparam, PressState::Released),
        WM_MOUSEWHEEL => captured_scroll(lparam, WindowsScrollAxis::Vertical),
        WM_MOUSEHWHEEL => captured_scroll(lparam, WindowsScrollAxis::Horizontal),
        WM_KEYDOWN | WM_SYSKEYDOWN => captured_keyboard_input(lparam, PressState::Pressed),
        WM_KEYUP | WM_SYSKEYUP => captured_keyboard_input(lparam, PressState::Released),
        _ => None,
    }
}

#[cfg(windows)]
fn captured_mouse_button(
    lparam: LPARAM,
    button: MouseButton,
    state: PressState,
) -> Option<CapturedInputEvent> {
    let mouse = windows_mouse_hook_data(lparam)?;
    if mouse.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
        return None;
    }

    Some(CapturedInputEvent::MouseButton { button, state })
}

#[cfg(windows)]
fn captured_pointer_move(
    lparam: LPARAM,
    state: &mut WindowsCaptureThreadState,
) -> Option<CapturedInputEvent> {
    let mouse = windows_mouse_hook_data(lparam)?;
    if mouse.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
        return None;
    }

    let next = (mouse.pt.x, mouse.pt.y);
    let previous = state.last_pointer.replace(next)?;
    let delta_x = next.0.saturating_sub(previous.0);
    let delta_y = next.1.saturating_sub(previous.1);

    if delta_x == 0 && delta_y == 0 {
        None
    } else {
        Some(CapturedInputEvent::PointerMoved { delta_x, delta_y })
    }
}

#[cfg(windows)]
fn captured_x_button(lparam: LPARAM, state: PressState) -> Option<CapturedInputEvent> {
    let mouse = windows_mouse_hook_data(lparam)?;
    if mouse.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
        return None;
    }

    let button = match ((mouse.mouseData >> 16) & 0xffff) as u16 {
        XBUTTON1 => MouseButton::Back,
        XBUTTON2 => MouseButton::Forward,
        other => MouseButton::Other(other),
    };

    Some(CapturedInputEvent::MouseButton { button, state })
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsScrollAxis {
    Horizontal,
    Vertical,
}

#[cfg(windows)]
fn captured_scroll(lparam: LPARAM, axis: WindowsScrollAxis) -> Option<CapturedInputEvent> {
    let mouse = windows_mouse_hook_data(lparam)?;

    captured_scroll_from_mouse(mouse, axis)
}

#[cfg(windows)]
fn captured_scroll_from_mouse(
    mouse: MSLLHOOKSTRUCT,
    axis: WindowsScrollAxis,
) -> Option<CapturedInputEvent> {
    if mouse.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
        return None;
    }

    let delta = windows_wheel_delta_from_mouse_data(mouse.mouseData);
    if delta == 0 {
        return None;
    }

    match axis {
        WindowsScrollAxis::Horizontal => Some(CapturedInputEvent::Scroll {
            delta_x: delta,
            delta_y: 0,
        }),
        WindowsScrollAxis::Vertical => Some(CapturedInputEvent::Scroll {
            delta_x: 0,
            delta_y: delta,
        }),
    }
}

#[cfg(windows)]
fn windows_wheel_delta_from_mouse_data(mouse_data: u32) -> i32 {
    ((mouse_data >> 16) as u16 as i16) as i32
}

#[cfg(windows)]
fn captured_keyboard_input(lparam: LPARAM, state: PressState) -> Option<CapturedInputEvent> {
    let keyboard = windows_keyboard_hook_data(lparam)?;
    if keyboard.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
        return None;
    }

    Some(CapturedInputEvent::Key {
        key: physical_key_from_windows_hook(keyboard),
        state,
    })
}

#[cfg(windows)]
fn physical_key_from_windows_hook(keyboard: KBDLLHOOKSTRUCT) -> PhysicalKey {
    match keyboard.vkCode as u16 {
        VK_LSHIFT => PhysicalKey::LeftShift,
        VK_RSHIFT => PhysicalKey::RightShift,
        VK_LCONTROL => PhysicalKey::LeftControl,
        VK_RCONTROL => PhysicalKey::RightControl,
        VK_LMENU => PhysicalKey::LeftAlt,
        VK_RMENU => PhysicalKey::RightAlt,
        VK_LWIN => PhysicalKey::LeftMeta,
        VK_RWIN => PhysicalKey::RightMeta,
        VK_BACK => DEFAULT_PANIC_HOTKEY_KEY,
        _ => PhysicalKey::Code(keyboard.scanCode as u16),
    }
}

#[cfg(windows)]
fn windows_mouse_hook_data(lparam: LPARAM) -> Option<MSLLHOOKSTRUCT> {
    if lparam == 0 {
        return None;
    }

    // SAFETY: For mouse hook messages, lparam points to an MSLLHOOKSTRUCT for the duration of the
    // callback. The value is copied immediately and not retained.
    Some(unsafe { *(lparam as *const MSLLHOOKSTRUCT) })
}

#[cfg(windows)]
fn windows_keyboard_hook_data(lparam: LPARAM) -> Option<KBDLLHOOKSTRUCT> {
    if lparam == 0 {
        return None;
    }

    // SAFETY: For keyboard hook messages, lparam points to a KBDLLHOOKSTRUCT for the duration of
    // the callback. The value is copied immediately and not retained.
    Some(unsafe { *(lparam as *const KBDLLHOOKSTRUCT) })
}

#[cfg(windows)]
fn can_send_zero_delta_mouse_input() -> bool {
    send_windows_mouse_move(0, 0).is_ok()
}

#[cfg(windows)]
fn inject_windows_input(
    injected_state: &Mutex<WindowsInjectedInputState>,
    event: &InjectedInputEvent,
) -> Result<(), PlatformError> {
    match event {
        InjectedInputEvent::PointerMoved { delta_x, delta_y } => {
            send_windows_mouse_move(*delta_x, *delta_y)
        }
        InjectedInputEvent::Scroll { delta_x, delta_y } => send_windows_scroll(*delta_x, *delta_y),
        InjectedInputEvent::Key { key, state } => {
            send_windows_keyboard_input(*key, *state)?;
            update_windows_injected_state(injected_state, WindowsPressedInput::Key(*key), *state)
        }
        InjectedInputEvent::MouseButton { button, state } => {
            send_windows_mouse_button_input(*button, *state)?;
            update_windows_injected_state(
                injected_state,
                WindowsPressedInput::MouseButton(*button),
                *state,
            )
        }
    }
}

#[cfg(windows)]
fn release_all_windows_inputs(
    injected_state: &Mutex<WindowsInjectedInputState>,
) -> Result<(), PlatformError> {
    let mut state = injected_state
        .lock()
        .map_err(|_| PlatformError::new("windows injected input state lock was poisoned"))?;

    for input in state.release_sequence() {
        match input {
            WindowsPressedInput::Key(key) => send_windows_keyboard_input(key, PressState::Released),
            WindowsPressedInput::MouseButton(button) => {
                send_windows_mouse_button_input(button, PressState::Released)
            }
        }?;
        state.mark_released(input);
    }

    Ok(())
}

#[cfg(windows)]
fn update_windows_injected_state(
    injected_state: &Mutex<WindowsInjectedInputState>,
    input: WindowsPressedInput,
    press_state: PressState,
) -> Result<(), PlatformError> {
    let mut state = injected_state
        .lock()
        .map_err(|_| PlatformError::new("windows injected input state lock was poisoned"))?;

    match press_state {
        PressState::Pressed => state.mark_pressed(input),
        PressState::Released => state.mark_released(input),
    }

    Ok(())
}

#[cfg(windows)]
fn send_windows_mouse_move(delta_x: i32, delta_y: i32) -> Result<(), PlatformError> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: delta_x,
                dy: delta_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    // SAFETY: input points to one initialized INPUT value and cbSize matches the INPUT ABI size.
    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };

    if sent == 1 {
        Ok(())
    } else {
        Err(PlatformError::new("failed to send Windows mouse input"))
    }
}

#[cfg(windows)]
fn send_windows_scroll(delta_x: i32, delta_y: i32) -> Result<(), PlatformError> {
    if delta_y != 0 {
        send_windows_mouse_wheel_input(WindowsScrollAxis::Vertical, delta_y)?;
    }
    if delta_x != 0 {
        send_windows_mouse_wheel_input(WindowsScrollAxis::Horizontal, delta_x)?;
    }

    Ok(())
}

#[cfg(windows)]
fn send_windows_keyboard_input(
    key: PhysicalKey,
    press_state: PressState,
) -> Result<(), PlatformError> {
    send_windows_input(
        windows_input_for_keyboard(key, press_state),
        "failed to send Windows keyboard input",
    )
}

#[cfg(windows)]
fn send_windows_mouse_button_input(
    button: MouseButton,
    press_state: PressState,
) -> Result<(), PlatformError> {
    send_windows_input(
        windows_input_for_mouse_button(button, press_state)?,
        "failed to send Windows mouse button input",
    )
}

#[cfg(windows)]
fn send_windows_mouse_wheel_input(
    axis: WindowsScrollAxis,
    delta: i32,
) -> Result<(), PlatformError> {
    send_windows_input(
        windows_input_for_mouse_wheel(axis, delta),
        "failed to send Windows mouse wheel input",
    )
}

#[cfg(windows)]
fn send_windows_input(input: INPUT, failure_message: &'static str) -> Result<(), PlatformError> {
    // SAFETY: input points to one initialized INPUT value and cbSize matches the INPUT ABI size.
    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };

    if sent == 1 {
        Ok(())
    } else {
        Err(PlatformError::new(failure_message))
    }
}

#[cfg(windows)]
fn windows_input_for_keyboard(key: PhysicalKey, press_state: PressState) -> INPUT {
    let (scan_code, extended) = windows_scan_code_for_physical_key(key);
    let mut flags = KEYEVENTF_SCANCODE;
    if press_state == PressState::Released {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(windows)]
fn windows_input_for_mouse_wheel(axis: WindowsScrollAxis, delta: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta as u32,
                dwFlags: windows_mouse_wheel_flags(axis),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(windows)]
fn windows_mouse_wheel_flags(axis: WindowsScrollAxis) -> u32 {
    match axis {
        WindowsScrollAxis::Horizontal => MOUSEEVENTF_HWHEEL,
        WindowsScrollAxis::Vertical => MOUSEEVENTF_WHEEL,
    }
}

#[cfg(windows)]
fn windows_scan_code_for_physical_key(key: PhysicalKey) -> (u16, bool) {
    match key {
        PhysicalKey::LeftShift => (0x2a, false),
        PhysicalKey::RightShift => (0x36, false),
        PhysicalKey::LeftControl => (0x1d, false),
        PhysicalKey::RightControl => (0x1d, true),
        PhysicalKey::LeftAlt => (0x38, false),
        PhysicalKey::RightAlt => (0x38, true),
        PhysicalKey::LeftMeta => (0x5b, true),
        PhysicalKey::RightMeta => (0x5c, true),
        PhysicalKey::Code(code) => (code, false),
    }
}

#[cfg(windows)]
fn windows_input_for_mouse_button(
    button: MouseButton,
    press_state: PressState,
) -> Result<INPUT, PlatformError> {
    let (flags, mouse_data) = windows_mouse_button_flags(button, press_state)?;

    Ok(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

#[cfg(windows)]
fn windows_mouse_button_flags(
    button: MouseButton,
    press_state: PressState,
) -> Result<(u32, u32), PlatformError> {
    let flags = match (button, press_state) {
        (MouseButton::Left, PressState::Pressed) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, PressState::Released) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, PressState::Pressed) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, PressState::Released) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, PressState::Pressed) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, PressState::Released) => MOUSEEVENTF_MIDDLEUP,
        (MouseButton::Back | MouseButton::Forward, PressState::Pressed) => MOUSEEVENTF_XDOWN,
        (MouseButton::Back | MouseButton::Forward, PressState::Released) => MOUSEEVENTF_XUP,
        (MouseButton::Other(_), _) => {
            return Err(PlatformError::new(
                "unsupported Windows mouse button for injection",
            ));
        }
    };
    let mouse_data = match button {
        MouseButton::Back => XBUTTON1 as u32,
        MouseButton::Forward => XBUTTON2 as u32,
        _ => 0,
    };

    Ok((flags, mouse_data))
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsPressedInput {
    Key(PhysicalKey),
    MouseButton(MouseButton),
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct WindowsInjectedInputState {
    pressed_keys: BTreeSet<PhysicalKey>,
    pressed_buttons: BTreeSet<MouseButton>,
    press_order: Vec<WindowsPressedInput>,
}

#[cfg(windows)]
impl WindowsInjectedInputState {
    fn mark_pressed(&mut self, input: WindowsPressedInput) {
        let inserted = match input {
            WindowsPressedInput::Key(key) => self.pressed_keys.insert(key),
            WindowsPressedInput::MouseButton(button) => self.pressed_buttons.insert(button),
        };

        if inserted {
            self.press_order.push(input);
        }
    }

    fn mark_released(&mut self, input: WindowsPressedInput) {
        match input {
            WindowsPressedInput::Key(key) => {
                self.pressed_keys.remove(&key);
            }
            WindowsPressedInput::MouseButton(button) => {
                self.pressed_buttons.remove(&button);
            }
        }
        self.press_order.retain(|pressed| *pressed != input);
    }

    fn release_sequence(&self) -> Vec<WindowsPressedInput> {
        self.press_order.iter().rev().copied().collect()
    }
}

/// Adapter used on operating systems that do not have an implementation yet.
#[cfg(any(not(windows), test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedPlatformAdapter {
    session: UnsupportedDesktopSession,
}

#[cfg(any(not(windows), test))]
impl UnsupportedPlatformAdapter {
    /// Create the unsupported platform adapter.
    pub fn new() -> Self {
        Self {
            session: detect_unsupported_desktop_session(),
        }
    }

    #[cfg(test)]
    fn from_session(session: UnsupportedDesktopSession) -> Self {
        Self { session }
    }
}

#[cfg(any(not(windows), test))]
impl Default for UnsupportedPlatformAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(not(windows), test))]
impl PlatformAdapter for UnsupportedPlatformAdapter {
    fn name(&self) -> &'static str {
        self.session.adapter_name()
    }

    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        Ok(PlatformCapabilities::default())
    }

    fn diagnostic_permission_issues(&self) -> Vec<PlatformDiagnosticIssue> {
        self.session.diagnostic_permission_issues()
    }

    fn release_all(&self) -> Result<(), PlatformError> {
        Ok(())
    }
}

#[cfg(any(not(windows), test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsupportedDesktopSession {
    LinuxX11,
    LinuxWayland,
    LinuxUnknown,
    Macos,
    Other,
}

#[cfg(any(not(windows), test))]
impl UnsupportedDesktopSession {
    fn adapter_name(self) -> &'static str {
        match self {
            Self::LinuxX11 => "linux-x11",
            Self::LinuxWayland => "linux-wayland",
            Self::LinuxUnknown => "linux-unknown",
            Self::Macos => "macos-unsupported",
            Self::Other => "unsupported",
        }
    }

    fn diagnostic_permission_issues(self) -> Vec<PlatformDiagnosticIssue> {
        match self {
            Self::LinuxX11 => linux_x11_diagnostic_permission_issues(),
            Self::LinuxWayland => LINUX_WAYLAND_DIAGNOSTIC_ISSUES.to_vec(),
            Self::LinuxUnknown => LINUX_UNKNOWN_DIAGNOSTIC_ISSUES.to_vec(),
            Self::Macos => MACOS_DIAGNOSTIC_ISSUES.to_vec(),
            Self::Other => UNSUPPORTED_PLATFORM_DIAGNOSTIC_ISSUES.to_vec(),
        }
    }
}

#[cfg(any(not(windows), test))]
const LINUX_X11_DIAGNOSTIC_ISSUES: [PlatformDiagnosticIssue; 2] = [
    PlatformDiagnosticIssue {
        code: "linux_x11_capture_unimplemented",
        message: concat!(
            "Linux X11 input capture is disabled for this build; ",
            "XInput2 capture and Xrandr layout probes are required before capture can be enabled.",
        ),
    },
    PlatformDiagnosticIssue {
        code: "linux_x11_injection_unimplemented",
        message: concat!(
            "Linux X11 input injection is disabled for this build; ",
            "XTEST is available, and pointer, button, scroll, and keyboard injection handlers are required next.",
        ),
    },
];

#[cfg(any(not(windows), test))]
fn linux_x11_diagnostic_permission_issues() -> Vec<PlatformDiagnosticIssue> {
    #[cfg(all(target_os = "linux", not(test)))]
    {
        linux_x11_diagnostic_issues_from_xtest_probe(probe_linux_x11_xtest_availability()).to_vec()
    }

    #[cfg(not(all(target_os = "linux", not(test))))]
    {
        LINUX_X11_DIAGNOSTIC_ISSUES.to_vec()
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxX11XtestProbeResult {
    Available,
    DisplayUnavailable,
    X11LibraryUnavailable,
    XtestLibraryUnavailable,
    ProbeSymbolUnavailable,
    ExtensionUnavailable,
}

#[cfg(any(target_os = "linux", test))]
fn linux_x11_diagnostic_issues_from_xtest_probe(
    result: LinuxX11XtestProbeResult,
) -> [PlatformDiagnosticIssue; 2] {
    [
        LINUX_X11_DIAGNOSTIC_ISSUES[0],
        linux_x11_injection_diagnostic_issue_from_xtest_probe(result),
    ]
}

#[cfg(any(target_os = "linux", test))]
fn linux_x11_injection_diagnostic_issue_from_xtest_probe(
    result: LinuxX11XtestProbeResult,
) -> PlatformDiagnosticIssue {
    match result {
        LinuxX11XtestProbeResult::Available => LINUX_X11_DIAGNOSTIC_ISSUES[1],
        LinuxX11XtestProbeResult::DisplayUnavailable => PlatformDiagnosticIssue {
            code: "linux_x11_injection_display_unavailable",
            message: concat!(
                "Linux X11 input injection cannot start because no X display connection could be opened; ",
                "check DISPLAY and X server access before enabling injection.",
            ),
        },
        LinuxX11XtestProbeResult::X11LibraryUnavailable => PlatformDiagnosticIssue {
            code: "linux_x11_injection_x11_library_unavailable",
            message: concat!(
                "Linux X11 input injection cannot verify XTEST because libX11 could not be loaded; ",
                "install the X11 runtime library before enabling injection.",
            ),
        },
        LinuxX11XtestProbeResult::XtestLibraryUnavailable => PlatformDiagnosticIssue {
            code: "linux_x11_injection_xtest_library_unavailable",
            message: concat!(
                "Linux X11 input injection cannot verify XTEST because libXtst could not be loaded; ",
                "install the XTEST runtime library before enabling injection.",
            ),
        },
        LinuxX11XtestProbeResult::ProbeSymbolUnavailable => PlatformDiagnosticIssue {
            code: "linux_x11_injection_xtest_probe_unavailable",
            message: concat!(
                "Linux X11 input injection cannot verify XTEST because required X11/XTEST symbols are missing; ",
                "install matching libX11 and libXtst runtime libraries before enabling injection.",
            ),
        },
        LinuxX11XtestProbeResult::ExtensionUnavailable => PlatformDiagnosticIssue {
            code: "linux_x11_injection_xtest_unavailable",
            message: concat!(
                "Linux X11 input injection cannot start because the XTEST extension is not available; ",
                "enable XTEST in the X server before enabling injection.",
            ),
        },
    }
}

#[cfg(all(target_os = "linux", not(test)))]
type XOpenDisplayFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
#[cfg(all(target_os = "linux", not(test)))]
type XCloseDisplayFn = unsafe extern "C" fn(*mut c_void) -> c_int;
#[cfg(all(target_os = "linux", not(test)))]
type XTestQueryExtensionFn =
    unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int, *mut c_int, *mut c_int) -> c_int;

#[cfg(all(target_os = "linux", not(test)))]
#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

#[cfg(all(target_os = "linux", not(test)))]
struct LinuxDynamicLibrary {
    handle: *mut c_void,
}

#[cfg(all(target_os = "linux", not(test)))]
impl LinuxDynamicLibrary {
    fn open_any(names: &[&[u8]]) -> Option<Self> {
        const RTLD_LAZY: c_int = 1;

        for name in names {
            // SAFETY: each name is a hard-coded NUL-terminated C string.
            let handle = unsafe { dlopen(name.as_ptr().cast(), RTLD_LAZY) };
            if !handle.is_null() {
                return Some(Self { handle });
            }
        }

        None
    }

    fn symbol_ptr(&self, symbol: &[u8]) -> Option<*mut c_void> {
        // SAFETY: symbol is a hard-coded NUL-terminated C string and handle is owned by self.
        let ptr = unsafe { dlsym(self.handle, symbol.as_ptr().cast()) };
        (!ptr.is_null()).then_some(ptr)
    }
}

#[cfg(all(target_os = "linux", not(test)))]
impl Drop for LinuxDynamicLibrary {
    fn drop(&mut self) {
        // SAFETY: handle was returned by dlopen and is closed exactly once in Drop.
        unsafe {
            dlclose(self.handle);
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
fn probe_linux_x11_xtest_availability() -> LinuxX11XtestProbeResult {
    let Some(x11) = LinuxDynamicLibrary::open_any(&[&b"libX11.so.6\0"[..], &b"libX11.so\0"[..]])
    else {
        return LinuxX11XtestProbeResult::X11LibraryUnavailable;
    };
    let Some(xtst) = LinuxDynamicLibrary::open_any(&[&b"libXtst.so.6\0"[..], &b"libXtst.so\0"[..]])
    else {
        return LinuxX11XtestProbeResult::XtestLibraryUnavailable;
    };

    let Some(open_display_ptr) = x11.symbol_ptr(b"XOpenDisplay\0") else {
        return LinuxX11XtestProbeResult::ProbeSymbolUnavailable;
    };
    let Some(close_display_ptr) = x11.symbol_ptr(b"XCloseDisplay\0") else {
        return LinuxX11XtestProbeResult::ProbeSymbolUnavailable;
    };
    let Some(query_extension_ptr) = xtst.symbol_ptr(b"XTestQueryExtension\0") else {
        return LinuxX11XtestProbeResult::ProbeSymbolUnavailable;
    };

    // SAFETY: symbols are loaded from libX11/libXtst and matched to their documented C ABIs.
    let open_display =
        unsafe { std::mem::transmute::<*mut c_void, XOpenDisplayFn>(open_display_ptr) };
    // SAFETY: symbols are loaded from libX11/libXtst and matched to their documented C ABIs.
    let close_display =
        unsafe { std::mem::transmute::<*mut c_void, XCloseDisplayFn>(close_display_ptr) };
    // SAFETY: symbols are loaded from libX11/libXtst and matched to their documented C ABIs.
    let query_extension =
        unsafe { std::mem::transmute::<*mut c_void, XTestQueryExtensionFn>(query_extension_ptr) };

    // SAFETY: null display name asks Xlib to use DISPLAY; Xlib returns null on failure.
    let display = unsafe { open_display(std::ptr::null()) };
    if display.is_null() {
        return LinuxX11XtestProbeResult::DisplayUnavailable;
    }

    let mut event_base = 0;
    let mut error_base = 0;
    let mut major_version = 0;
    let mut minor_version = 0;
    // SAFETY: display is a live Xlib display and the output pointers reference stack integers.
    let available = unsafe {
        query_extension(
            display,
            &mut event_base,
            &mut error_base,
            &mut major_version,
            &mut minor_version,
        ) != 0
    };
    // SAFETY: display was opened by XOpenDisplay above and is closed exactly once.
    unsafe {
        close_display(display);
    }

    if available {
        LinuxX11XtestProbeResult::Available
    } else {
        LinuxX11XtestProbeResult::ExtensionUnavailable
    }
}

#[cfg(any(not(windows), test))]
const LINUX_WAYLAND_DIAGNOSTIC_ISSUES: [PlatformDiagnosticIssue; 2] = [
    PlatformDiagnosticIssue {
        code: "linux_wayland_capture_unimplemented",
        message: "Linux Wayland input capture is not implemented yet; portal and compositor support must be verified before capture can be enabled.",
    },
    PlatformDiagnosticIssue {
        code: "linux_wayland_injection_unimplemented",
        message: "Linux Wayland input injection is not implemented yet; portal and compositor support must be verified before injection can be enabled.",
    },
];

#[cfg(any(not(windows), test))]
const LINUX_UNKNOWN_DIAGNOSTIC_ISSUES: [PlatformDiagnosticIssue; 2] = [
    PlatformDiagnosticIssue {
        code: "linux_session_capture_unknown",
        message: "Linux desktop session could not be detected; set XDG_SESSION_TYPE, DISPLAY, or WAYLAND_DISPLAY before checking capture support.",
    },
    PlatformDiagnosticIssue {
        code: "linux_session_injection_unknown",
        message: "Linux desktop session could not be detected; set XDG_SESSION_TYPE, DISPLAY, or WAYLAND_DISPLAY before checking injection support.",
    },
];

#[cfg(any(not(windows), test))]
const MACOS_DIAGNOSTIC_ISSUES: [PlatformDiagnosticIssue; 2] = [
    PlatformDiagnosticIssue {
        code: "macos_capture_unimplemented",
        message: "macOS input capture is not implemented yet; Accessibility permission and CGEventTap probes are required before capture can be enabled.",
    },
    PlatformDiagnosticIssue {
        code: "macos_injection_unimplemented",
        message: "macOS input injection is not implemented yet; Accessibility permission and CGEventPost probes are required before injection can be enabled.",
    },
];

#[cfg(any(not(windows), test))]
const UNSUPPORTED_PLATFORM_DIAGNOSTIC_ISSUES: [PlatformDiagnosticIssue; 2] = [
    PlatformDiagnosticIssue {
        code: "platform_capture_unimplemented",
        message: "This platform does not have an input capture adapter yet.",
    },
    PlatformDiagnosticIssue {
        code: "platform_injection_unimplemented",
        message: "This platform does not have an input injection adapter yet.",
    },
];

#[cfg(any(not(windows), test))]
fn detect_unsupported_desktop_session() -> UnsupportedDesktopSession {
    let xdg_session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let display = std::env::var("DISPLAY").ok();

    detect_unsupported_desktop_session_from_env(
        std::env::consts::OS,
        xdg_session_type.as_deref(),
        wayland_display.as_deref(),
        display.as_deref(),
    )
}

#[cfg(any(not(windows), test))]
fn detect_unsupported_desktop_session_from_env(
    target_os: &str,
    xdg_session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
) -> UnsupportedDesktopSession {
    match target_os {
        "linux" => detect_linux_desktop_session(xdg_session_type, wayland_display, display),
        "macos" => UnsupportedDesktopSession::Macos,
        _ => UnsupportedDesktopSession::Other,
    }
}

#[cfg(any(not(windows), test))]
fn detect_linux_desktop_session(
    xdg_session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
) -> UnsupportedDesktopSession {
    match normalized_env_value(xdg_session_type).as_deref() {
        Some("wayland") => return UnsupportedDesktopSession::LinuxWayland,
        Some("x11" | "xorg") => return UnsupportedDesktopSession::LinuxX11,
        _ => {}
    }

    if env_value_is_present(wayland_display) {
        return UnsupportedDesktopSession::LinuxWayland;
    }
    if env_value_is_present(display) {
        return UnsupportedDesktopSession::LinuxX11;
    }

    UnsupportedDesktopSession::LinuxUnknown
}

#[cfg(any(not(windows), test))]
fn normalized_env_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

#[cfg(any(not(windows), test))]
fn env_value_is_present(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| !value.is_empty())
}

/// Deterministic platform adapter for core and daemon tests.
#[derive(Debug, Clone)]
pub struct FakePlatformAdapter {
    capabilities: PlatformCapabilities,
    desktop_geometry: Arc<Mutex<DesktopGeometry>>,
    keyboard_layout: KeyboardLayoutSnapshot,
    captured_events: Vec<CapturedInputEvent>,
    capture_policy: SharedInputCapturePolicy,
    keep_capture_session_open: bool,
    open_capture_senders: Arc<Mutex<Vec<SyncSender<CapturedInputEvent>>>>,
    injected_events: Arc<Mutex<Vec<InjectedInputEvent>>>,
    release_all_count: Arc<Mutex<u64>>,
}

impl FakePlatformAdapter {
    /// Create a fake adapter with the supplied capability profile.
    pub fn new(capabilities: PlatformCapabilities) -> Self {
        Self {
            capabilities,
            desktop_geometry: Arc::new(Mutex::new(fake_desktop_geometry())),
            keyboard_layout: fake_keyboard_layout(),
            captured_events: Vec::new(),
            capture_policy: SharedInputCapturePolicy::default(),
            keep_capture_session_open: false,
            open_capture_senders: Arc::new(Mutex::new(Vec::new())),
            injected_events: Arc::new(Mutex::new(Vec::new())),
            release_all_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Return a fake adapter that reports the supplied desktop geometry.
    pub fn with_desktop_geometry(mut self, desktop_geometry: DesktopGeometry) -> Self {
        self.desktop_geometry = Arc::new(Mutex::new(desktop_geometry));
        self
    }

    /// Replace the desktop geometry reported by this fake adapter.
    pub fn set_desktop_geometry(
        &self,
        desktop_geometry: DesktopGeometry,
    ) -> Result<(), PlatformError> {
        let mut current = self
            .desktop_geometry
            .lock()
            .map_err(|_| PlatformError::new("fake platform desktop geometry lock was poisoned"))?;
        *current = desktop_geometry;

        Ok(())
    }

    /// Return a fake adapter that reports the supplied keyboard layout.
    pub fn with_keyboard_layout(mut self, keyboard_layout: KeyboardLayoutSnapshot) -> Self {
        self.keyboard_layout = keyboard_layout;
        self
    }

    /// Return a fake adapter that preloads captured input events when capture starts.
    pub fn with_captured_events(
        mut self,
        events: impl IntoIterator<Item = CapturedInputEvent>,
    ) -> Self {
        self.captured_events = events.into_iter().collect();
        self
    }

    /// Return a fake adapter whose capture session stays connected without new events.
    pub fn with_open_input_capture(mut self) -> Self {
        self.keep_capture_session_open = true;
        self
    }

    /// Return how many times `release_all` has been requested.
    pub fn release_all_count(&self) -> Result<u64, PlatformError> {
        let count = self
            .release_all_count
            .lock()
            .map_err(|_| PlatformError::new("fake platform release counter lock was poisoned"))?;

        Ok(*count)
    }

    /// Return the input events requested through `inject_input`.
    pub fn injected_events(&self) -> Result<Vec<InjectedInputEvent>, PlatformError> {
        self.injected_events
            .lock()
            .map_err(|_| PlatformError::new("fake platform injection log lock was poisoned"))
            .map(|events| events.clone())
    }

    /// Return the current capture policy observed by fake capture sessions.
    pub fn input_capture_policy(&self) -> InputCapturePolicy {
        self.capture_policy.load()
    }
}

impl Default for FakePlatformAdapter {
    fn default() -> Self {
        Self::new(PlatformCapabilities::all_enabled())
    }
}

impl PlatformAdapter for FakePlatformAdapter {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        Ok(self.capabilities.clone())
    }

    fn read_desktop_geometry(&self) -> Result<DesktopGeometry, PlatformError> {
        self.desktop_geometry
            .lock()
            .map_err(|_| PlatformError::new("fake platform desktop geometry lock was poisoned"))
            .map(|geometry| geometry.clone())
    }

    fn read_keyboard_layout(&self) -> Result<KeyboardLayoutSnapshot, PlatformError> {
        Ok(self.keyboard_layout.clone())
    }

    fn start_input_capture(
        &self,
        config: InputCaptureConfig,
    ) -> Result<InputCaptureSession, PlatformError> {
        let (sender, receiver) = sync_channel(config.bounded_capacity());
        for event in self.captured_events.clone() {
            if sender.try_send(event).is_err() {
                break;
            }
        }

        if self.keep_capture_session_open {
            self.open_capture_senders
                .lock()
                .map_err(|_| PlatformError::new("fake platform capture sender lock was poisoned"))?
                .push(sender);
        } else {
            drop(sender);
        }

        Ok(InputCaptureSession::new(
            receiver,
            InputCaptureShutdown::Noop,
            self.capture_policy.clone(),
        ))
    }

    fn inject_input(&self, event: &InjectedInputEvent) -> Result<(), PlatformError> {
        self.injected_events
            .lock()
            .map_err(|_| PlatformError::new("fake platform injection log lock was poisoned"))?
            .push(event.clone());

        Ok(())
    }

    fn release_all(&self) -> Result<(), PlatformError> {
        let mut count = self
            .release_all_count
            .lock()
            .map_err(|_| PlatformError::new("fake platform release counter lock was poisoned"))?;
        *count += 1;

        Ok(())
    }
}

fn fake_desktop_geometry() -> DesktopGeometry {
    DesktopGeometry {
        pointer_position: LogicalPoint { x: 0, y: 0 },
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

fn fake_keyboard_layout() -> KeyboardLayoutSnapshot {
    KeyboardLayoutSnapshot {
        source: "fake".to_string(),
        layout_id: "0x0000000004090409".to_string(),
        language_id: "0x0409".to_string(),
        layout_name: Some("00000409".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
    use std::time::Duration;

    #[cfg(windows)]
    use akraz_core::MouseButton;
    use akraz_core::{
        CapturedInputEvent, InjectedInputEvent, LogicalPoint, LogicalRect, LogicalSize,
        PhysicalKey, PressState,
    };

    use super::{
        DesktopGeometry, DesktopMonitor, FakePlatformAdapter, InputCaptureConfig,
        InputCapturePolicy, KeyboardLayoutSnapshot, LinuxX11XtestProbeResult, PlatformAdapter,
        PlatformCapabilities, PlatformError, UnsupportedDesktopSession, UnsupportedPlatformAdapter,
        detect_unsupported_desktop_session_from_env, linux_x11_diagnostic_issues_from_xtest_probe,
        runtime_platform_adapter,
    };

    #[cfg(windows)]
    use super::{
        HID_USAGE_GENERIC_KEYBOARD, HID_USAGE_GENERIC_MOUSE, HID_USAGE_PAGE_GENERIC, HWND,
        LLMHF_INJECTED, MOUSE_MOVE_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_WHEEL, MOUSEEVENTF_XUP, MSLLHOOKSTRUCT, POINT, RAWINPUTDEVICE, RAWKEYBOARD,
        RAWMOUSE, RI_KEY_BREAK, RI_KEY_E0, RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_WHEEL,
        RIDEV_INPUTSINK, VK_CONTROL, VK_SHIFT, WindowsCaptureThreadState,
        WindowsInjectedInputState, WindowsPressedInput, WindowsProbeResults, WindowsScrollAxis,
        captured_event_from_windows_raw_keyboard, captured_events_from_windows_raw_mouse,
        captured_scroll_from_mouse, raw_mouse_button_data_delta,
        windows_capabilities_from_probe_results, windows_mouse_button_flags,
        windows_mouse_wheel_flags, windows_raw_input_devices, windows_scan_code_for_physical_key,
        windows_wheel_delta_from_mouse_data,
    };
    #[cfg(windows)]
    use windows_sys::Win32::UI::Input::{RAWMOUSE_0, RAWMOUSE_0_0};
    #[cfg(windows)]
    use windows_sys::Win32::UI::WindowsAndMessaging::WHEEL_DELTA;

    fn count_or_panic(adapter: &FakePlatformAdapter) -> u64 {
        match adapter.release_all_count() {
            Ok(count) => count,
            Err(error) => panic!("expected fake platform count: {error}"),
        }
    }

    #[test]
    fn platform_error_displays_message() {
        let error = PlatformError::new("permission missing");

        assert_eq!(error.to_string(), "permission missing");
    }

    #[test]
    fn default_capabilities_are_closed() {
        assert_eq!(
            PlatformCapabilities::default(),
            PlatformCapabilities {
                can_capture_pointer: false,
                can_capture_keyboard: false,
                can_inject_pointer: false,
                can_inject_keyboard: false,
            }
        );
    }

    #[test]
    fn all_enabled_capabilities_enable_every_input_surface() {
        assert_eq!(
            PlatformCapabilities::all_enabled(),
            PlatformCapabilities {
                can_capture_pointer: true,
                can_capture_keyboard: true,
                can_inject_pointer: true,
                can_inject_keyboard: true,
            }
        );
    }

    #[test]
    fn linux_desktop_session_detection_prefers_declared_session_type() {
        assert_eq!(
            detect_unsupported_desktop_session_from_env(
                "linux",
                Some("wayland"),
                Some("wayland-0"),
                Some(":0"),
            ),
            UnsupportedDesktopSession::LinuxWayland
        );
        assert_eq!(
            detect_unsupported_desktop_session_from_env(
                "linux",
                Some("x11"),
                Some("wayland-0"),
                Some(":0"),
            ),
            UnsupportedDesktopSession::LinuxX11
        );
    }

    #[test]
    fn linux_desktop_session_detection_uses_display_fallbacks() {
        assert_eq!(
            detect_unsupported_desktop_session_from_env("linux", None, Some("wayland-0"), None),
            UnsupportedDesktopSession::LinuxWayland
        );
        assert_eq!(
            detect_unsupported_desktop_session_from_env("linux", Some(""), None, Some(":0")),
            UnsupportedDesktopSession::LinuxX11
        );
        assert_eq!(
            detect_unsupported_desktop_session_from_env("linux", Some("tty"), None, None),
            UnsupportedDesktopSession::LinuxUnknown
        );
    }

    #[test]
    fn unsupported_desktop_sessions_have_stable_adapter_names() {
        assert_eq!(
            UnsupportedDesktopSession::LinuxX11.adapter_name(),
            "linux-x11"
        );
        assert_eq!(
            UnsupportedDesktopSession::LinuxWayland.adapter_name(),
            "linux-wayland"
        );
        assert_eq!(
            UnsupportedDesktopSession::LinuxUnknown.adapter_name(),
            "linux-unknown"
        );
        assert_eq!(
            UnsupportedDesktopSession::Macos.adapter_name(),
            "macos-unsupported"
        );
        assert_eq!(
            UnsupportedDesktopSession::Other.adapter_name(),
            "unsupported"
        );
    }

    #[test]
    fn unsupported_adapter_reports_session_without_opening_capabilities() {
        let adapter =
            UnsupportedPlatformAdapter::from_session(UnsupportedDesktopSession::LinuxWayland);

        assert_eq!(adapter.name(), "linux-wayland");
        assert_eq!(
            adapter.probe_capabilities(),
            Ok(PlatformCapabilities::default())
        );
        assert_eq!(
            match adapter.start_input_capture(InputCaptureConfig::default()) {
                Ok(_) => panic!("unsupported capture should fail"),
                Err(error) => error.to_string(),
            },
            "linux-wayland input capture is not available",
        );
        assert_eq!(adapter.release_all(), Ok(()));
    }

    #[test]
    fn unsupported_adapter_reports_session_permission_diagnostics() {
        let linux_x11 =
            UnsupportedPlatformAdapter::from_session(UnsupportedDesktopSession::LinuxX11);
        let linux_unknown =
            UnsupportedPlatformAdapter::from_session(UnsupportedDesktopSession::LinuxUnknown);
        let linux_x11_issues = linux_x11.diagnostic_permission_issues();

        assert_eq!(
            linux_x11_issues
                .iter()
                .map(|issue| issue.code)
                .collect::<Vec<_>>(),
            vec![
                "linux_x11_capture_unimplemented",
                "linux_x11_injection_unimplemented",
            ]
        );
        assert!(linux_x11_issues[0].message.contains("XInput2 capture"));
        assert!(linux_x11_issues[0].message.contains("Xrandr layout probes"));
        assert!(linux_x11_issues[1].message.contains("XTEST is available"));
        assert_eq!(
            linux_unknown
                .diagnostic_permission_issues()
                .iter()
                .map(|issue| issue.code)
                .collect::<Vec<_>>(),
            vec![
                "linux_session_capture_unknown",
                "linux_session_injection_unknown",
            ]
        );
    }

    #[test]
    fn linux_x11_diagnostics_report_xtest_probe_state() {
        let available =
            linux_x11_diagnostic_issues_from_xtest_probe(LinuxX11XtestProbeResult::Available);

        assert_eq!(available[1].code, "linux_x11_injection_unimplemented");
        assert!(available[1].message.contains("XTEST is available"));

        let missing_display = linux_x11_diagnostic_issues_from_xtest_probe(
            LinuxX11XtestProbeResult::DisplayUnavailable,
        );

        assert_eq!(
            missing_display[1].code,
            "linux_x11_injection_display_unavailable",
        );
        assert!(missing_display[1].message.contains("DISPLAY"));

        let missing_x11_library = linux_x11_diagnostic_issues_from_xtest_probe(
            LinuxX11XtestProbeResult::X11LibraryUnavailable,
        );

        assert_eq!(
            missing_x11_library[1].code,
            "linux_x11_injection_x11_library_unavailable",
        );
        assert!(missing_x11_library[1].message.contains("libX11"));

        let missing_xtest_library = linux_x11_diagnostic_issues_from_xtest_probe(
            LinuxX11XtestProbeResult::XtestLibraryUnavailable,
        );

        assert_eq!(
            missing_xtest_library[1].code,
            "linux_x11_injection_xtest_library_unavailable",
        );
        assert!(missing_xtest_library[1].message.contains("libXtst"));

        let missing_probe_symbol = linux_x11_diagnostic_issues_from_xtest_probe(
            LinuxX11XtestProbeResult::ProbeSymbolUnavailable,
        );

        assert_eq!(
            missing_probe_symbol[1].code,
            "linux_x11_injection_xtest_probe_unavailable",
        );
        assert!(
            missing_probe_symbol[1]
                .message
                .contains("required X11/XTEST symbols")
        );

        let missing_extension = linux_x11_diagnostic_issues_from_xtest_probe(
            LinuxX11XtestProbeResult::ExtensionUnavailable,
        );

        assert_eq!(
            missing_extension[1].code,
            "linux_x11_injection_xtest_unavailable",
        );
        assert!(missing_extension[1].message.contains("XTEST extension"));
    }

    #[test]
    fn fake_adapter_reports_capabilities_and_counts_release_all() {
        let capabilities = PlatformCapabilities {
            can_capture_pointer: true,
            can_capture_keyboard: false,
            can_inject_pointer: true,
            can_inject_keyboard: false,
        };
        let adapter = FakePlatformAdapter::new(capabilities.clone());

        assert_eq!(adapter.name(), "fake");
        assert_eq!(adapter.probe_capabilities(), Ok(capabilities));
        assert_eq!(count_or_panic(&adapter), 0);

        assert_eq!(adapter.release_all(), Ok(()));
        assert_eq!(adapter.release_all(), Ok(()));

        assert_eq!(count_or_panic(&adapter), 2);
    }

    #[test]
    fn fake_adapter_reports_configured_desktop_geometry() {
        let geometry = DesktopGeometry {
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
                scale_factor_percent: Some(125),
                is_primary: true,
            }],
        };
        let adapter = FakePlatformAdapter::default().with_desktop_geometry(geometry.clone());

        assert_eq!(adapter.read_desktop_geometry(), Ok(geometry));
    }

    #[test]
    fn fake_adapter_updates_desktop_geometry_between_reads() {
        let initial_geometry = DesktopGeometry {
            pointer_position: LogicalPoint { x: 0, y: 0 },
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
        };
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
        let adapter =
            FakePlatformAdapter::default().with_desktop_geometry(initial_geometry.clone());

        assert_eq!(adapter.read_desktop_geometry(), Ok(initial_geometry));
        assert_eq!(
            adapter.set_desktop_geometry(updated_geometry.clone()),
            Ok(())
        );
        assert_eq!(adapter.read_desktop_geometry(), Ok(updated_geometry));
    }

    #[test]
    fn fake_adapter_reports_configured_keyboard_layout() {
        let keyboard_layout = KeyboardLayoutSnapshot {
            source: "test".to_string(),
            layout_id: "0x0000000004120412".to_string(),
            language_id: "0x0412".to_string(),
            layout_name: Some("00000412".to_string()),
        };
        let adapter = FakePlatformAdapter::default().with_keyboard_layout(keyboard_layout.clone());

        assert_eq!(adapter.read_keyboard_layout(), Ok(keyboard_layout));
    }

    #[test]
    fn fake_adapter_preloads_bounded_capture_events() {
        let adapter = FakePlatformAdapter::default().with_captured_events(vec![
            CapturedInputEvent::Key {
                key: PhysicalKey::LeftShift,
                state: PressState::Pressed,
            },
            CapturedInputEvent::Key {
                key: PhysicalKey::LeftShift,
                state: PressState::Released,
            },
        ]);
        let session = adapter
            .start_input_capture(InputCaptureConfig {
                event_buffer_capacity: 1,
            })
            .expect("fake capture session");

        assert_eq!(
            session.try_recv(),
            Ok(CapturedInputEvent::Key {
                key: PhysicalKey::LeftShift,
                state: PressState::Pressed,
            })
        );
        assert_eq!(session.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn fake_adapter_capture_policy_follows_capture_session_updates() {
        let adapter = FakePlatformAdapter::default();
        let session = adapter
            .start_input_capture(InputCaptureConfig::default())
            .expect("fake capture session");

        assert_eq!(session.policy(), InputCapturePolicy::PassThrough);
        assert_eq!(
            adapter.input_capture_policy(),
            InputCapturePolicy::PassThrough
        );

        session.set_policy(InputCapturePolicy::ConsumeCapturedInput);

        assert_eq!(session.policy(), InputCapturePolicy::ConsumeCapturedInput);
        assert_eq!(
            adapter.input_capture_policy(),
            InputCapturePolicy::ConsumeCapturedInput
        );
    }

    #[test]
    fn fake_adapter_can_keep_capture_session_open_without_events() {
        let adapter = FakePlatformAdapter::default().with_open_input_capture();
        let session = adapter
            .start_input_capture(InputCaptureConfig::default())
            .expect("fake capture session");

        assert_eq!(
            session.recv_timeout(Duration::from_millis(1)),
            Err(RecvTimeoutError::Timeout)
        );

        session.stop().expect("stop fake capture session");
    }

    #[test]
    fn fake_adapter_records_injected_input_events() {
        let adapter = FakePlatformAdapter::default();

        assert_eq!(
            adapter.inject_input(&InjectedInputEvent::PointerMoved {
                delta_x: 8,
                delta_y: 2,
            }),
            Ok(())
        );
        assert_eq!(
            adapter.inject_input(&InjectedInputEvent::Scroll {
                delta_x: 0,
                delta_y: -120,
            }),
            Ok(())
        );

        assert_eq!(
            adapter.injected_events().expect("fake injected events"),
            vec![
                InjectedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: 2,
                },
                InjectedInputEvent::Scroll {
                    delta_x: 0,
                    delta_y: -120,
                },
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_scan_code_mapping_preserves_extended_modifier_keys() {
        assert_eq!(
            windows_scan_code_for_physical_key(PhysicalKey::LeftControl),
            (0x1d, false)
        );
        assert_eq!(
            windows_scan_code_for_physical_key(PhysicalKey::RightControl),
            (0x1d, true)
        );
        assert_eq!(
            windows_scan_code_for_physical_key(PhysicalKey::Code(44)),
            (44, false)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mouse_button_flags_cover_primary_and_x_buttons() {
        assert_eq!(
            windows_mouse_button_flags(MouseButton::Left, PressState::Pressed),
            Ok((MOUSEEVENTF_LEFTDOWN, 0))
        );
        assert_eq!(
            windows_mouse_button_flags(MouseButton::Forward, PressState::Released),
            Ok((MOUSEEVENTF_XUP, super::XBUTTON2 as u32))
        );

        let error = windows_mouse_button_flags(MouseButton::Other(9), PressState::Pressed)
            .expect_err("unknown mouse button should not be injected");
        assert_eq!(
            error.to_string(),
            "unsupported Windows mouse button for injection"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_scroll_flags_cover_vertical_and_horizontal_wheels() {
        assert_eq!(
            windows_mouse_wheel_flags(WindowsScrollAxis::Vertical),
            MOUSEEVENTF_WHEEL
        );
        assert_eq!(
            windows_mouse_wheel_flags(WindowsScrollAxis::Horizontal),
            MOUSEEVENTF_HWHEEL
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_scroll_capture_normalizes_wheel_delta_and_ignores_injected_events() {
        let vertical = MSLLHOOKSTRUCT {
            pt: POINT { x: 0, y: 0 },
            mouseData: WHEEL_DELTA << 16,
            flags: 0,
            time: 0,
            dwExtraInfo: 0,
        };
        let horizontal = MSLLHOOKSTRUCT {
            pt: POINT { x: 0, y: 0 },
            mouseData: (-(WHEEL_DELTA as i16) as u16 as u32) << 16,
            flags: 0,
            time: 0,
            dwExtraInfo: 0,
        };
        let injected = MSLLHOOKSTRUCT {
            flags: LLMHF_INJECTED,
            ..vertical
        };

        assert_eq!(
            windows_wheel_delta_from_mouse_data(vertical.mouseData),
            WHEEL_DELTA as i32
        );
        assert_eq!(
            captured_scroll_from_mouse(vertical, WindowsScrollAxis::Vertical),
            Some(CapturedInputEvent::Scroll {
                delta_x: 0,
                delta_y: WHEEL_DELTA as i32,
            })
        );
        assert_eq!(
            captured_scroll_from_mouse(horizontal, WindowsScrollAxis::Horizontal),
            Some(CapturedInputEvent::Scroll {
                delta_x: -(WHEEL_DELTA as i32),
                delta_y: 0,
            })
        );
        assert_eq!(
            captured_scroll_from_mouse(injected, WindowsScrollAxis::Vertical),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_raw_input_registration_targets_hidden_window_for_mouse_and_keyboard() {
        let hwnd = 1usize as HWND;
        let devices: [RAWINPUTDEVICE; 2] = windows_raw_input_devices(hwnd);

        assert_eq!(devices[0].usUsagePage, HID_USAGE_PAGE_GENERIC);
        assert_eq!(devices[0].usUsage, HID_USAGE_GENERIC_MOUSE);
        assert_eq!(devices[0].dwFlags, RIDEV_INPUTSINK);
        assert_eq!(devices[0].hwndTarget, hwnd);
        assert_eq!(devices[1].usUsagePage, HID_USAGE_PAGE_GENERIC);
        assert_eq!(devices[1].usUsage, HID_USAGE_GENERIC_KEYBOARD);
        assert_eq!(devices[1].dwFlags, RIDEV_INPUTSINK);
        assert_eq!(devices[1].hwndTarget, hwnd);
    }

    #[cfg(windows)]
    #[test]
    fn windows_raw_mouse_events_preserve_move_button_and_scroll_order() {
        let mut state = WindowsCaptureThreadState::default();
        let mouse = RAWMOUSE {
            usFlags: 0,
            Anonymous: RAWMOUSE_0 {
                Anonymous: RAWMOUSE_0_0 {
                    usButtonFlags: (RI_MOUSE_LEFT_BUTTON_DOWN | RI_MOUSE_WHEEL) as u16,
                    usButtonData: WHEEL_DELTA as u16,
                },
            },
            ulRawButtons: 0,
            lLastX: 8,
            lLastY: -3,
            ulExtraInformation: 0,
        };

        assert_eq!(
            captured_events_from_windows_raw_mouse(mouse, &mut state),
            vec![
                CapturedInputEvent::PointerMoved {
                    delta_x: 8,
                    delta_y: -3,
                },
                CapturedInputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: PressState::Pressed,
                },
                CapturedInputEvent::Scroll {
                    delta_x: 0,
                    delta_y: WHEEL_DELTA as i32,
                },
            ]
        );
        assert_eq!(
            raw_mouse_button_data_delta((-(WHEEL_DELTA as i16)) as u16),
            -(WHEEL_DELTA as i32)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_raw_absolute_mouse_normalizes_against_previous_position() {
        let mut state = WindowsCaptureThreadState::default();
        let first = RAWMOUSE {
            usFlags: MOUSE_MOVE_ABSOLUTE,
            Anonymous: RAWMOUSE_0 {
                Anonymous: RAWMOUSE_0_0 {
                    usButtonFlags: 0,
                    usButtonData: 0,
                },
            },
            ulRawButtons: 0,
            lLastX: 100,
            lLastY: 120,
            ulExtraInformation: 0,
        };
        let second = RAWMOUSE {
            lLastX: 115,
            lLastY: 110,
            ..first
        };

        assert!(captured_events_from_windows_raw_mouse(first, &mut state).is_empty());
        assert_eq!(
            captured_events_from_windows_raw_mouse(second, &mut state),
            vec![CapturedInputEvent::PointerMoved {
                delta_x: 15,
                delta_y: -10,
            }]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_raw_keyboard_maps_side_specific_modifiers_and_release_state() {
        let right_control = RAWKEYBOARD {
            MakeCode: 0x1d,
            Flags: RI_KEY_E0 as u16,
            Reserved: 0,
            VKey: VK_CONTROL,
            Message: 0,
            ExtraInformation: 0,
        };
        let right_shift_release = RAWKEYBOARD {
            MakeCode: 0x36,
            Flags: RI_KEY_BREAK as u16,
            Reserved: 0,
            VKey: VK_SHIFT,
            Message: 0,
            ExtraInformation: 0,
        };

        assert_eq!(
            captured_event_from_windows_raw_keyboard(right_control),
            Some(CapturedInputEvent::Key {
                key: PhysicalKey::RightControl,
                state: PressState::Pressed,
            })
        );
        assert_eq!(
            captured_event_from_windows_raw_keyboard(right_shift_release),
            Some(CapturedInputEvent::Key {
                key: PhysicalKey::RightShift,
                state: PressState::Released,
            })
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_injected_input_state_releases_in_reverse_press_order() {
        let mut state = WindowsInjectedInputState::default();
        let shift = WindowsPressedInput::Key(PhysicalKey::LeftShift);
        let button = WindowsPressedInput::MouseButton(MouseButton::Left);

        state.mark_pressed(shift);
        state.mark_pressed(button);
        state.mark_pressed(shift);

        assert_eq!(state.release_sequence(), vec![button, shift]);

        state.mark_released(button);

        assert_eq!(state.release_sequence(), vec![shift]);
    }

    #[test]
    fn runtime_adapter_reports_current_platform() {
        let adapter = runtime_platform_adapter();

        if cfg!(windows) {
            assert_eq!(adapter.name(), "windows");
        } else if cfg!(target_os = "linux") {
            assert!(matches!(
                adapter.name(),
                "linux-x11" | "linux-wayland" | "linux-unknown"
            ));
        } else if cfg!(target_os = "macos") {
            assert_eq!(adapter.name(), "macos-unsupported");
        } else {
            assert_eq!(adapter.name(), "unsupported");
        }
        if !cfg!(windows) {
            assert_eq!(
                adapter.probe_capabilities(),
                Ok(PlatformCapabilities::default())
            );
        }
        assert_eq!(adapter.release_all(), Ok(()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_adapter_reports_real_desktop_geometry() {
        let adapter = runtime_platform_adapter();
        let geometry = adapter.read_desktop_geometry().expect("desktop geometry");

        assert!(geometry.virtual_screen_bounds.is_valid());
        assert!(
            geometry
                .virtual_screen_bounds
                .contains(geometry.pointer_position)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_capabilities_map_send_input_probe_to_injection_surfaces() {
        let capabilities = windows_capabilities_from_probe_results(WindowsProbeResults {
            can_open_input_desktop: true,
            can_install_mouse_hook: true,
            can_install_keyboard_hook: true,
            can_send_mouse_input: true,
        });

        assert_eq!(
            capabilities,
            PlatformCapabilities {
                can_capture_pointer: true,
                can_capture_keyboard: true,
                can_inject_pointer: true,
                can_inject_keyboard: true,
            }
        );

        let blocked_desktop = windows_capabilities_from_probe_results(WindowsProbeResults {
            can_open_input_desktop: false,
            can_install_mouse_hook: true,
            can_install_keyboard_hook: true,
            can_send_mouse_input: true,
        });

        assert_eq!(
            blocked_desktop,
            PlatformCapabilities {
                can_capture_pointer: false,
                can_capture_keyboard: false,
                can_inject_pointer: true,
                can_inject_keyboard: true,
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_adapter_reports_keyboard_injection_with_send_input_profile() {
        let adapter = runtime_platform_adapter();

        let capabilities = adapter.probe_capabilities().expect("capability probe");

        assert_eq!(
            capabilities.can_inject_keyboard,
            capabilities.can_inject_pointer
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_input_capture_session_starts_and_stops_when_capture_is_available() {
        let adapter = runtime_platform_adapter();
        let capabilities = adapter.probe_capabilities().expect("capability probe");
        if !capabilities.can_capture_pointer || !capabilities.can_capture_keyboard {
            return;
        }

        let session = adapter
            .start_input_capture(InputCaptureConfig {
                event_buffer_capacity: 4,
            })
            .expect("windows capture session");

        session.stop().expect("stop capture session");
    }
}
