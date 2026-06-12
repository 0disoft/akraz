//! Platform adapter contract shared by OS-specific akraz crates.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Mutex;

/// Capabilities reported by a platform adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformCapabilities {
    pub can_capture_pointer: bool,
    pub can_capture_keyboard: bool,
    pub can_inject_pointer: bool,
    pub can_inject_keyboard: bool,
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

/// OS-independent platform adapter interface.
pub trait PlatformAdapter {
    /// Stable adapter name, such as `windows` or `fake`.
    fn name(&self) -> &'static str;

    /// Probe platform input capabilities.
    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError>;

    /// Release all currently pressed keys and buttons known to the adapter.
    fn release_all(&self) -> Result<(), PlatformError>;
}

/// Runtime platform adapter selected for the current operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsPlatformAdapter;

#[cfg(windows)]
impl WindowsPlatformAdapter {
    /// Create the Windows platform adapter.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
impl PlatformAdapter for WindowsPlatformAdapter {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        Ok(PlatformCapabilities::default())
    }

    fn release_all(&self) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// Adapter used on operating systems that do not have an implementation yet.
#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnsupportedPlatformAdapter;

#[cfg(not(windows))]
impl UnsupportedPlatformAdapter {
    /// Create the unsupported platform adapter.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(windows))]
impl PlatformAdapter for UnsupportedPlatformAdapter {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError> {
        Ok(PlatformCapabilities::default())
    }

    fn release_all(&self) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// Deterministic platform adapter for core and daemon tests.
#[derive(Debug)]
pub struct FakePlatformAdapter {
    capabilities: PlatformCapabilities,
    release_all_count: Mutex<u64>,
}

impl FakePlatformAdapter {
    /// Create a fake adapter with the supplied capability profile.
    pub fn new(capabilities: PlatformCapabilities) -> Self {
        Self {
            capabilities,
            release_all_count: Mutex::new(0),
        }
    }

    /// Return how many times `release_all` has been requested.
    pub fn release_all_count(&self) -> Result<u64, PlatformError> {
        let count = self
            .release_all_count
            .lock()
            .map_err(|_| PlatformError::new("fake platform release counter lock was poisoned"))?;

        Ok(*count)
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

    fn release_all(&self) -> Result<(), PlatformError> {
        let mut count = self
            .release_all_count
            .lock()
            .map_err(|_| PlatformError::new("fake platform release counter lock was poisoned"))?;
        *count += 1;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FakePlatformAdapter, PlatformAdapter, PlatformCapabilities, PlatformError,
        runtime_platform_adapter,
    };

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
    fn runtime_adapter_reports_current_platform_and_closed_capabilities() {
        let adapter = runtime_platform_adapter();

        if cfg!(windows) {
            assert_eq!(adapter.name(), "windows");
        } else {
            assert_eq!(adapter.name(), "unsupported");
        }
        assert_eq!(
            adapter.probe_capabilities(),
            Ok(PlatformCapabilities::default())
        );
        assert_eq!(adapter.release_all(), Ok(()));
    }
}
