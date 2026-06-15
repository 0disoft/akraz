//! Wire protocol contracts for akraz peer sessions.

use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

/// First supported protocol major version.
pub const PROTOCOL_MAJOR: u16 = 1;

/// First supported protocol minor version.
pub const PROTOCOL_MINOR: u16 = 0;

/// Label included in the signed v1 session authentication transcript.
pub const AUTH_TRANSCRIPT_LABEL: &str = "akraz-auth-v1";

/// TLS exporter label used for v1 session channel binding.
pub const SESSION_TLS_EXPORTER_LABEL: &str = "akraz/session/v1";

/// Fixed nonce size for v1 session handshakes.
pub const HANDSHAKE_NONCE_LEN: usize = 32;

/// Fixed TLS exporter size mixed into the v1 authentication transcript.
pub const TLS_EXPORTER_LEN: usize = 32;

/// Protocol version exchanged during session negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    /// Current protocol version.
    pub const CURRENT: Self = Self {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    };

    /// Return whether this version can share a session with `other`.
    pub fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major
    }

    /// Negotiate a shared protocol version.
    pub fn negotiate(self, other: Self) -> Result<Self, ProtocolNegotiationError> {
        if !self.is_compatible_with(other) {
            return Err(ProtocolNegotiationError::MajorMismatch {
                local: self,
                remote: other,
            });
        }

        Ok(Self {
            major: self.major,
            minor: self.minor.min(other.minor),
        })
    }

    /// Negotiate a peer version against the current protocol version.
    pub fn negotiate_with_current(remote: Self) -> Result<Self, ProtocolNegotiationError> {
        Self::CURRENT.negotiate(remote)
    }
}

/// Failure returned when two peers cannot agree on a protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolNegotiationError {
    MajorMismatch {
        local: ProtocolVersion,
        remote: ProtocolVersion,
    },
}

impl Display for ProtocolNegotiationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MajorMismatch { local, remote } => write!(
                formatter,
                "unsupported protocol major version: local {}.{}, remote {}.{}",
                local.major, local.minor, remote.major, remote.minor
            ),
        }
    }
}

impl Error for ProtocolNegotiationError {}

/// Capabilities exchanged during session setup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityFlags(u32);

impl CapabilityFlags {
    pub const POINTER: Self = Self(1 << 0);
    pub const KEYBOARD: Self = Self(1 << 1);
    pub const CLIPBOARD: Self = Self(1 << 2);
    pub const SCREEN_LAYOUT: Self = Self(1 << 3);

    const KNOWN_BITS: u32 =
        Self::POINTER.0 | Self::KEYBOARD.0 | Self::CLIPBOARD.0 | Self::SCREEN_LAYOUT.0;

    /// Empty capability set.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Return a capability set from trusted in-process bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return a capability set from wire bits, ignoring unknown future flags.
    pub const fn from_wire_bits(bits: u32) -> Self {
        Self(bits & Self::KNOWN_BITS)
    }

    /// Return the raw capability bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Return whether all `other` capabilities are present.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Return the shared capabilities between two peers.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Return whether this capability set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for CapabilityFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for CapabilityFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for CapabilityFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        self.intersection(rhs)
    }
}

/// Role bound into the authentication transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PeerRole {
    Initiator,
    Responder,
}

/// First frame in the authenticated peer-session handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: ProtocolVersion,
    pub device_id: String,
    pub display_name: String,
    pub build_version: String,
    pub capabilities: CapabilityFlags,
    pub nonce: [u8; HANDSHAKE_NONCE_LEN],
}

/// Signed proof that binds peer identity to the encrypted transport channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProof {
    pub device_id: String,
    pub role: PeerRole,
    pub signature: Vec<u8>,
}

/// Handshake completion frame that permits input event processing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReady {
    pub session_id: String,
    pub sequence_base: u64,
    pub capabilities: CapabilityFlags,
}

/// Canonical inputs that must be signed for v1 session authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthTranscript {
    pub local_device_id: String,
    pub remote_device_id: String,
    pub local_nonce: [u8; HANDSHAKE_NONCE_LEN],
    pub remote_nonce: [u8; HANDSHAKE_NONCE_LEN],
    pub protocol: ProtocolVersion,
    pub local_capabilities: CapabilityFlags,
    pub remote_capabilities: CapabilityFlags,
    pub role: PeerRole,
    pub tls_exporter: [u8; TLS_EXPORTER_LEN],
}

impl AuthTranscript {
    /// Return the fixed transcript label included in signature input.
    pub const fn label(&self) -> &'static str {
        AUTH_TRANSCRIPT_LABEL
    }
}

/// Peer-session handshake message envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HandshakeMessage {
    Hello(Hello),
    AuthProof(AuthProof),
    SessionReady(SessionReady),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AUTH_TRANSCRIPT_LABEL, AuthProof, AuthTranscript, CapabilityFlags, HANDSHAKE_NONCE_LEN,
        HandshakeMessage, Hello, PeerRole, ProtocolNegotiationError, ProtocolVersion,
        SESSION_TLS_EXPORTER_LABEL, SessionReady, TLS_EXPORTER_LEN,
    };

    #[test]
    fn major_version_controls_compatibility() {
        assert!(
            ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion {
                major: 1,
                minor: 99
            })
        );
        assert!(
            !ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion { major: 2, minor: 0 })
        );
    }

    #[test]
    fn negotiation_uses_lowest_supported_minor_version() {
        assert_eq!(
            ProtocolVersion::CURRENT.negotiate(ProtocolVersion {
                major: 1,
                minor: 99
            }),
            Ok(ProtocolVersion { major: 1, minor: 0 })
        );
        assert_eq!(
            ProtocolVersion { major: 1, minor: 7 }
                .negotiate(ProtocolVersion { major: 1, minor: 3 }),
            Ok(ProtocolVersion { major: 1, minor: 3 })
        );
    }

    #[test]
    fn negotiation_rejects_different_major_versions() {
        assert_eq!(
            ProtocolVersion::CURRENT.negotiate(ProtocolVersion { major: 2, minor: 0 }),
            Err(ProtocolNegotiationError::MajorMismatch {
                local: ProtocolVersion::CURRENT,
                remote: ProtocolVersion { major: 2, minor: 0 },
            })
        );
    }

    #[test]
    fn capability_flags_intersect_and_ignore_unknown_wire_bits() {
        let local = CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD;
        let remote = CapabilityFlags::from_wire_bits(
            CapabilityFlags::POINTER.bits() | CapabilityFlags::CLIPBOARD.bits() | (1 << 31),
        );

        assert!(remote.contains(CapabilityFlags::POINTER));
        assert!(!remote.contains(CapabilityFlags::KEYBOARD));
        assert_eq!(
            remote.bits(),
            CapabilityFlags::POINTER.bits() | CapabilityFlags::CLIPBOARD.bits()
        );
        assert_eq!(local.intersection(remote), CapabilityFlags::POINTER);
    }

    #[test]
    fn handshake_messages_use_camel_case_wire_contract() {
        let hello = Hello {
            protocol: ProtocolVersion::CURRENT,
            device_id: "device-a".to_string(),
            display_name: "Device A".to_string(),
            build_version: "0.4.20".to_string(),
            capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            nonce: [7; HANDSHAKE_NONCE_LEN],
        };

        let value = serde_json::to_value(HandshakeMessage::Hello(hello)).expect("hello JSON");

        assert_eq!(
            value,
            json!({
                "kind": "hello",
                "protocol": { "major": 1, "minor": 0 },
                "deviceId": "device-a",
                "displayName": "Device A",
                "buildVersion": "0.4.20",
                "capabilities": 3,
                "nonce": [7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
            })
        );
    }

    #[test]
    fn auth_proof_and_session_ready_wire_shapes_are_stable() {
        assert_eq!(
            serde_json::to_value(HandshakeMessage::AuthProof(AuthProof {
                device_id: "device-a".to_string(),
                role: PeerRole::Initiator,
                signature: vec![1, 2, 3],
            }))
            .expect("auth proof JSON"),
            json!({
                "kind": "authProof",
                "deviceId": "device-a",
                "role": "initiator",
                "signature": [1, 2, 3]
            })
        );
        assert_eq!(
            serde_json::to_value(HandshakeMessage::SessionReady(SessionReady {
                session_id: "session-1".to_string(),
                sequence_base: 42,
                capabilities: CapabilityFlags::POINTER,
            }))
            .expect("session ready JSON"),
            json!({
                "kind": "sessionReady",
                "sessionId": "session-1",
                "sequenceBase": 42,
                "capabilities": 1
            })
        );
    }

    #[test]
    fn auth_transcript_binds_role_and_channel_binding_label() {
        let transcript = AuthTranscript {
            local_device_id: "device-a".to_string(),
            remote_device_id: "device-b".to_string(),
            local_nonce: [1; HANDSHAKE_NONCE_LEN],
            remote_nonce: [2; HANDSHAKE_NONCE_LEN],
            protocol: ProtocolVersion::CURRENT,
            local_capabilities: CapabilityFlags::POINTER,
            remote_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            role: PeerRole::Initiator,
            tls_exporter: [3; TLS_EXPORTER_LEN],
        };
        let reflected = AuthTranscript {
            role: PeerRole::Responder,
            ..transcript.clone()
        };

        assert_eq!(transcript.label(), AUTH_TRANSCRIPT_LABEL);
        assert_eq!(SESSION_TLS_EXPORTER_LABEL, "akraz/session/v1");
        assert_ne!(transcript, reflected);
    }
}
