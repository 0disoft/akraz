//! Core state and decision logic for akraz.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

/// Stable peer identifier supplied by discovery or pairing code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(String);

impl PeerId {
    /// Create a peer identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the identifier as a borrowed string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable remote-control session identifier supplied by the transport layer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Create a session identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the identifier as a borrowed string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Current local control mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ControlMode {
    /// Input remains on the local machine.
    #[default]
    Local,
    /// The state machine is waiting for the transport to confirm remote capture.
    EnteringRemote,
    /// Input is currently routed to a remote peer.
    Remote,
    /// The state machine is restoring local control.
    LeavingRemote,
    /// Input routing is paused while the system recovers from transport loss.
    Suspended,
}

/// Physical keyboard keys tracked by the core state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PhysicalKey {
    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftAlt,
    RightAlt,
    LeftMeta,
    RightMeta,
    Code(u16),
}

/// Mouse buttons tracked by the core state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Press or release state for a key or button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressState {
    Pressed,
    Released,
}

/// Currently pressed modifier keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModifierState {
    pub left_shift: bool,
    pub right_shift: bool,
    pub left_control: bool,
    pub right_control: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub left_meta: bool,
    pub right_meta: bool,
}

impl ModifierState {
    fn update_key(&mut self, key: PhysicalKey, state: PressState) {
        let is_pressed = state == PressState::Pressed;

        match key {
            PhysicalKey::LeftShift => self.left_shift = is_pressed,
            PhysicalKey::RightShift => self.right_shift = is_pressed,
            PhysicalKey::LeftControl => self.left_control = is_pressed,
            PhysicalKey::RightControl => self.right_control = is_pressed,
            PhysicalKey::LeftAlt => self.left_alt = is_pressed,
            PhysicalKey::RightAlt => self.right_alt = is_pressed,
            PhysicalKey::LeftMeta => self.left_meta = is_pressed,
            PhysicalKey::RightMeta => self.right_meta = is_pressed,
            PhysicalKey::Code(_) => {}
        }
    }
}

/// Captured local input facts normalized before platform-specific forwarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturedInputEvent {
    Key {
        key: PhysicalKey,
        state: PressState,
    },
    MouseButton {
        button: MouseButton,
        state: PressState,
    },
    PointerMoved {
        delta_x: i32,
        delta_y: i32,
    },
}

/// Runtime facts consumed by the core state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    Input(CapturedInputEvent),
    RemoteEntryRequested { peer_id: PeerId },
    RemoteEntryConfirmed { session_id: SessionId },
    RemoteLeaveRequested,
    LocalControlConfirmed,
    TransportLost,
    RecoveryCompleted,
}

/// Side effects the imperative shell must perform after a successful transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreAction {
    StartRemoteSession { peer_id: PeerId },
    ForwardInput { event: CapturedInputEvent },
    ReleaseAllInputs,
    StopRemoteSession { session_id: Option<SessionId> },
}

/// Expected transition failures returned by the core state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreTransitionError {
    InvalidTransition {
        mode: ControlMode,
        event: &'static str,
    },
}

impl Display for CoreTransitionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTransition { mode, event } => {
                write!(formatter, "invalid transition from {mode:?} with {event}")
            }
        }
    }
}

impl Error for CoreTransitionError {}

/// Input routing state owned by the daemon runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeInputState {
    mode: ControlMode,
    pending_peer_id: Option<PeerId>,
    active_peer_id: Option<PeerId>,
    active_session_id: Option<SessionId>,
    pressed_keys: BTreeSet<PhysicalKey>,
    pressed_buttons: BTreeSet<MouseButton>,
    modifiers: ModifierState,
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

    /// Return the peer currently being entered, if any.
    pub fn pending_peer_id(&self) -> Option<&PeerId> {
        self.pending_peer_id.as_ref()
    }

    /// Return the peer currently receiving input, if any.
    pub fn active_peer_id(&self) -> Option<&PeerId> {
        self.active_peer_id.as_ref()
    }

    /// Return the active remote-control session, if any.
    pub fn active_session_id(&self) -> Option<&SessionId> {
        self.active_session_id.as_ref()
    }

    /// Return the currently pressed physical keys.
    pub fn pressed_keys(&self) -> &BTreeSet<PhysicalKey> {
        &self.pressed_keys
    }

    /// Return the currently pressed mouse buttons.
    pub fn pressed_buttons(&self) -> &BTreeSet<MouseButton> {
        &self.pressed_buttons
    }

    /// Return the currently pressed modifier state.
    pub fn modifiers(&self) -> ModifierState {
        self.modifiers
    }

    /// Apply one runtime event and return the shell actions required by the transition.
    pub fn apply_event(
        &mut self,
        event: RuntimeEvent,
    ) -> Result<Vec<CoreAction>, CoreTransitionError> {
        match event {
            RuntimeEvent::Input(input) => Ok(self.apply_input(input)),
            RuntimeEvent::RemoteEntryRequested { peer_id } => self.request_remote_entry(peer_id),
            RuntimeEvent::RemoteEntryConfirmed { session_id } => {
                self.confirm_remote_entry(session_id)
            }
            RuntimeEvent::RemoteLeaveRequested => self.request_remote_leave(),
            RuntimeEvent::LocalControlConfirmed => self.confirm_local_control(),
            RuntimeEvent::TransportLost => Ok(self.handle_transport_lost()),
            RuntimeEvent::RecoveryCompleted => self.complete_recovery(),
        }
    }

    fn apply_input(&mut self, input: CapturedInputEvent) -> Vec<CoreAction> {
        self.record_input(&input);

        if self.mode == ControlMode::Remote {
            vec![CoreAction::ForwardInput { event: input }]
        } else {
            Vec::new()
        }
    }

    fn request_remote_entry(
        &mut self,
        peer_id: PeerId,
    ) -> Result<Vec<CoreAction>, CoreTransitionError> {
        if self.mode != ControlMode::Local {
            return Err(CoreTransitionError::InvalidTransition {
                mode: self.mode,
                event: "RemoteEntryRequested",
            });
        }

        self.mode = ControlMode::EnteringRemote;
        self.pending_peer_id = Some(peer_id.clone());

        Ok(vec![CoreAction::StartRemoteSession { peer_id }])
    }

    fn confirm_remote_entry(
        &mut self,
        session_id: SessionId,
    ) -> Result<Vec<CoreAction>, CoreTransitionError> {
        if self.mode != ControlMode::EnteringRemote {
            return Err(CoreTransitionError::InvalidTransition {
                mode: self.mode,
                event: "RemoteEntryConfirmed",
            });
        }

        let Some(peer_id) = self.pending_peer_id.take() else {
            return Err(CoreTransitionError::InvalidTransition {
                mode: self.mode,
                event: "RemoteEntryConfirmed",
            });
        };

        self.mode = ControlMode::Remote;
        self.active_peer_id = Some(peer_id);
        self.active_session_id = Some(session_id);

        Ok(Vec::new())
    }

    fn request_remote_leave(&mut self) -> Result<Vec<CoreAction>, CoreTransitionError> {
        if self.mode != ControlMode::Remote && self.mode != ControlMode::EnteringRemote {
            return Err(CoreTransitionError::InvalidTransition {
                mode: self.mode,
                event: "RemoteLeaveRequested",
            });
        }

        let mut actions = Vec::new();

        if self.has_pressed_input() || self.active_peer_id.is_some() {
            actions.push(CoreAction::ReleaseAllInputs);
        }

        let session_id = self.active_session_id.take();
        actions.push(CoreAction::StopRemoteSession { session_id });

        self.clear_input();
        self.pending_peer_id = None;
        self.active_peer_id = None;
        self.mode = ControlMode::LeavingRemote;

        Ok(actions)
    }

    fn confirm_local_control(&mut self) -> Result<Vec<CoreAction>, CoreTransitionError> {
        if self.mode != ControlMode::LeavingRemote {
            return Err(CoreTransitionError::InvalidTransition {
                mode: self.mode,
                event: "LocalControlConfirmed",
            });
        }

        self.mode = ControlMode::Local;

        Ok(Vec::new())
    }

    fn handle_transport_lost(&mut self) -> Vec<CoreAction> {
        let should_release = self.has_pressed_input()
            || self.pending_peer_id.is_some()
            || self.active_peer_id.is_some()
            || self.active_session_id.is_some();

        self.clear_input();
        self.pending_peer_id = None;
        self.active_peer_id = None;
        self.active_session_id = None;
        self.mode = ControlMode::Suspended;

        if should_release {
            vec![CoreAction::ReleaseAllInputs]
        } else {
            Vec::new()
        }
    }

    fn complete_recovery(&mut self) -> Result<Vec<CoreAction>, CoreTransitionError> {
        if self.mode != ControlMode::Suspended {
            return Err(CoreTransitionError::InvalidTransition {
                mode: self.mode,
                event: "RecoveryCompleted",
            });
        }

        self.mode = ControlMode::Local;

        Ok(Vec::new())
    }

    fn record_input(&mut self, input: &CapturedInputEvent) {
        match input {
            CapturedInputEvent::Key { key, state } => {
                match state {
                    PressState::Pressed => {
                        self.pressed_keys.insert(*key);
                    }
                    PressState::Released => {
                        self.pressed_keys.remove(key);
                    }
                }

                self.modifiers.update_key(*key, *state);
            }
            CapturedInputEvent::MouseButton { button, state } => match state {
                PressState::Pressed => {
                    self.pressed_buttons.insert(*button);
                }
                PressState::Released => {
                    self.pressed_buttons.remove(button);
                }
            },
            CapturedInputEvent::PointerMoved { .. } => {}
        }
    }

    fn clear_input(&mut self) {
        self.pressed_keys.clear();
        self.pressed_buttons.clear();
        self.modifiers = ModifierState::default();
    }

    fn has_pressed_input(&self) -> bool {
        !self.pressed_keys.is_empty() || !self.pressed_buttons.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CapturedInputEvent, ControlMode, CoreAction, CoreTransitionError, MouseButton, PeerId,
        PhysicalKey, PressState, RuntimeEvent, RuntimeInputState, SessionId,
    };

    fn apply_ok(state: &mut RuntimeInputState, event: RuntimeEvent) -> Vec<CoreAction> {
        match state.apply_event(event) {
            Ok(actions) => actions,
            Err(error) => panic!("expected transition to succeed: {error}"),
        }
    }

    #[test]
    fn starts_in_local_mode_with_empty_input() {
        let state = RuntimeInputState::new();

        assert_eq!(state.mode(), ControlMode::Local);
        assert!(state.pending_peer_id().is_none());
        assert!(state.active_peer_id().is_none());
        assert!(state.active_session_id().is_none());
        assert!(state.pressed_keys().is_empty());
        assert!(state.pressed_buttons().is_empty());
        assert_eq!(state.modifiers(), Default::default());
    }

    #[test]
    fn tracks_pressed_keys_buttons_and_modifiers() {
        let mut state = RuntimeInputState::new();

        assert!(
            apply_ok(
                &mut state,
                RuntimeEvent::Input(CapturedInputEvent::Key {
                    key: PhysicalKey::LeftShift,
                    state: PressState::Pressed,
                }),
            )
            .is_empty()
        );
        assert!(
            apply_ok(
                &mut state,
                RuntimeEvent::Input(CapturedInputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: PressState::Pressed,
                }),
            )
            .is_empty()
        );

        assert!(state.pressed_keys().contains(&PhysicalKey::LeftShift));
        assert!(state.pressed_buttons().contains(&MouseButton::Left));
        assert!(state.modifiers().left_shift);

        assert!(
            apply_ok(
                &mut state,
                RuntimeEvent::Input(CapturedInputEvent::Key {
                    key: PhysicalKey::LeftShift,
                    state: PressState::Released,
                }),
            )
            .is_empty()
        );

        assert!(!state.pressed_keys().contains(&PhysicalKey::LeftShift));
        assert!(!state.modifiers().left_shift);
    }

    #[test]
    fn remote_entry_moves_through_entering_then_remote() {
        let mut state = RuntimeInputState::new();
        let peer_id = PeerId::new("linux-laptop");
        let session_id = SessionId::new("session-1");

        let start_actions = apply_ok(
            &mut state,
            RuntimeEvent::RemoteEntryRequested {
                peer_id: peer_id.clone(),
            },
        );

        assert_eq!(state.mode(), ControlMode::EnteringRemote);
        assert_eq!(state.pending_peer_id(), Some(&peer_id));
        assert_eq!(
            start_actions,
            vec![CoreAction::StartRemoteSession {
                peer_id: peer_id.clone(),
            }]
        );

        let confirm_actions = apply_ok(
            &mut state,
            RuntimeEvent::RemoteEntryConfirmed {
                session_id: session_id.clone(),
            },
        );

        assert_eq!(state.mode(), ControlMode::Remote);
        assert_eq!(state.pending_peer_id(), None);
        assert_eq!(state.active_peer_id(), Some(&peer_id));
        assert_eq!(state.active_session_id(), Some(&session_id));
        assert!(confirm_actions.is_empty());
    }

    #[test]
    fn input_is_forwarded_only_while_remote() {
        let mut state = RuntimeInputState::new();

        assert!(
            apply_ok(
                &mut state,
                RuntimeEvent::Input(CapturedInputEvent::PointerMoved {
                    delta_x: 3,
                    delta_y: -2,
                }),
            )
            .is_empty()
        );

        apply_ok(
            &mut state,
            RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("windows-box"),
            },
        );
        apply_ok(
            &mut state,
            RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-2"),
            },
        );

        let input = CapturedInputEvent::PointerMoved {
            delta_x: 3,
            delta_y: -2,
        };
        let actions = apply_ok(&mut state, RuntimeEvent::Input(input.clone()));

        assert_eq!(actions, vec![CoreAction::ForwardInput { event: input }]);
    }

    #[test]
    fn transport_loss_releases_pressed_input_and_suspends() {
        let mut state = RuntimeInputState::new();

        apply_ok(
            &mut state,
            RuntimeEvent::Input(CapturedInputEvent::Key {
                key: PhysicalKey::LeftControl,
                state: PressState::Pressed,
            }),
        );
        apply_ok(
            &mut state,
            RuntimeEvent::Input(CapturedInputEvent::MouseButton {
                button: MouseButton::Right,
                state: PressState::Pressed,
            }),
        );

        let actions = apply_ok(&mut state, RuntimeEvent::TransportLost);

        assert_eq!(actions, vec![CoreAction::ReleaseAllInputs]);
        assert_eq!(state.mode(), ControlMode::Suspended);
        assert!(state.pressed_keys().is_empty());
        assert!(state.pressed_buttons().is_empty());
        assert_eq!(state.modifiers(), Default::default());
    }

    #[test]
    fn remote_leave_releases_input_and_waits_for_local_confirmation() {
        let mut state = RuntimeInputState::new();

        apply_ok(
            &mut state,
            RuntimeEvent::RemoteEntryRequested {
                peer_id: PeerId::new("linux-laptop"),
            },
        );
        apply_ok(
            &mut state,
            RuntimeEvent::RemoteEntryConfirmed {
                session_id: SessionId::new("session-3"),
            },
        );
        apply_ok(
            &mut state,
            RuntimeEvent::Input(CapturedInputEvent::Key {
                key: PhysicalKey::Code(44),
                state: PressState::Pressed,
            }),
        );

        let actions = apply_ok(&mut state, RuntimeEvent::RemoteLeaveRequested);

        assert_eq!(
            actions,
            vec![
                CoreAction::ReleaseAllInputs,
                CoreAction::StopRemoteSession {
                    session_id: Some(SessionId::new("session-3")),
                },
            ]
        );
        assert_eq!(state.mode(), ControlMode::LeavingRemote);
        assert!(state.active_peer_id().is_none());
        assert!(state.active_session_id().is_none());
        assert!(state.pressed_keys().is_empty());

        assert!(apply_ok(&mut state, RuntimeEvent::LocalControlConfirmed).is_empty());
        assert_eq!(state.mode(), ControlMode::Local);
    }

    #[test]
    fn invalid_transition_is_explicit_and_does_not_mutate_state() {
        let mut state = RuntimeInputState::new();
        let result = state.apply_event(RuntimeEvent::RemoteEntryConfirmed {
            session_id: SessionId::new("session-4"),
        });

        assert_eq!(
            result,
            Err(CoreTransitionError::InvalidTransition {
                mode: ControlMode::Local,
                event: "RemoteEntryConfirmed",
            })
        );
        assert_eq!(state.mode(), ControlMode::Local);
        assert!(state.active_session_id().is_none());
    }

    #[test]
    fn recovery_completed_returns_suspended_state_to_local() {
        let mut state = RuntimeInputState::new();

        apply_ok(&mut state, RuntimeEvent::TransportLost);
        assert_eq!(state.mode(), ControlMode::Suspended);

        assert!(apply_ok(&mut state, RuntimeEvent::RecoveryCompleted).is_empty());
        assert_eq!(state.mode(), ControlMode::Local);
    }
}
