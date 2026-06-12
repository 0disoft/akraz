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
    use super::{FakePlatformAdapter, PlatformAdapter, PlatformCapabilities, PlatformError};

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
}
