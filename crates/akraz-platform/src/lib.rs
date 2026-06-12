//! Platform adapter contract shared by OS-specific akraz crates.

use std::error::Error;
use std::fmt::{Display, Formatter};

/// Capabilities reported by a platform adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformCapabilities {
    pub can_capture_pointer: bool,
    pub can_capture_keyboard: bool,
    pub can_inject_pointer: bool,
    pub can_inject_keyboard: bool,
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
    /// Stable adapter name, such as `windows` or `dummy`.
    fn name(&self) -> &'static str;

    /// Probe platform input capabilities.
    fn probe_capabilities(&self) -> Result<PlatformCapabilities, PlatformError>;

    /// Release all currently pressed keys and buttons known to the adapter.
    fn release_all(&self) -> Result<(), PlatformError>;
}
