//! Core state and decision logic for akraz.

/// Current local control mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ControlMode {
    /// Input remains on the local machine.
    #[default]
    Local,
    /// The state machine is entering a remote peer.
    EnteringRemote,
    /// Input is currently routed to a remote peer.
    Remote,
    /// The state machine is restoring local control.
    LeavingRemote,
    /// Input routing is paused while the system recovers.
    Suspended,
}

/// Minimal input state used by the first daemon smoke target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeInputState {
    mode: ControlMode,
}

impl RuntimeInputState {
    /// Create a state machine in local mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the current control mode.
    pub fn mode(&self) -> ControlMode {
        self.mode
    }
}
