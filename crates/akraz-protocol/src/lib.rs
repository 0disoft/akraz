//! Wire protocol contracts for akraz peer sessions.

use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

/// First supported protocol major version.
pub const PROTOCOL_MAJOR: u16 = 1;

/// Current protocol minor version.
pub const PROTOCOL_MINOR: u16 = 2;

/// Label included in the signed v1 session authentication transcript.
pub const AUTH_TRANSCRIPT_LABEL: &str = "akraz-auth-v1";

/// TLS exporter label used for v1 session channel binding.
pub const SESSION_TLS_EXPORTER_LABEL: &str = "akraz/session/v1";

/// Fixed nonce size for v1 session handshakes.
pub const HANDSHAKE_NONCE_LEN: usize = 32;

/// Fixed TLS exporter size mixed into the v1 authentication transcript.
pub const TLS_EXPORTER_LEN: usize = 32;

/// Canonical binary format version for v1 authentication transcripts.
pub const AUTH_TRANSCRIPT_CANONICAL_VERSION: u8 = 1;

const AUTH_TRANSCRIPT_STRING_FIELD_MAX_LEN: usize = u16::MAX as usize;

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

    /// Encode this transcript into the canonical byte sequence used for signing.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AuthTranscriptEncodeError> {
        let mut bytes = Vec::with_capacity(self.canonical_capacity_hint());
        self.write_canonical(&mut bytes)?;
        Ok(bytes)
    }

    /// Append the canonical byte sequence used for signing to `output`.
    pub fn write_canonical(&self, output: &mut Vec<u8>) -> Result<(), AuthTranscriptEncodeError> {
        write_u8(output, AUTH_TRANSCRIPT_CANONICAL_VERSION);
        write_len_prefixed_str(output, "label", AUTH_TRANSCRIPT_LABEL)?;
        write_len_prefixed_str(output, "local device id", &self.local_device_id)?;
        write_len_prefixed_str(output, "remote device id", &self.remote_device_id)?;
        output.extend_from_slice(&self.local_nonce);
        output.extend_from_slice(&self.remote_nonce);
        write_u16(output, self.protocol.major);
        write_u16(output, self.protocol.minor);
        write_u32(output, self.local_capabilities.bits());
        write_u32(output, self.remote_capabilities.bits());
        write_u8(output, self.role.canonical_byte());
        output.extend_from_slice(&self.tls_exporter);

        Ok(())
    }

    fn canonical_capacity_hint(&self) -> usize {
        1 + 2
            + AUTH_TRANSCRIPT_LABEL.len()
            + 2
            + self.local_device_id.len()
            + 2
            + self.remote_device_id.len()
            + HANDSHAKE_NONCE_LEN
            + HANDSHAKE_NONCE_LEN
            + 2
            + 2
            + 4
            + 4
            + 1
            + TLS_EXPORTER_LEN
    }
}

/// Boundary used by identity-key implementations to sign a canonical auth transcript.
pub trait AuthProofSigner {
    type Error;

    fn sign_auth_transcript(&self, canonical_transcript: &[u8]) -> Result<Vec<u8>, Self::Error>;
}

/// Boundary used by identity-key implementations to verify a canonical auth transcript.
pub trait AuthProofVerifier {
    type Error;

    fn verify_auth_transcript(
        &self,
        canonical_transcript: &[u8],
        signature: &[u8],
    ) -> Result<(), Self::Error>;
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

impl AuthProof {
    /// Sign a canonical auth transcript and build an `AuthProof` frame.
    pub fn sign_transcript<S>(
        device_id: impl Into<String>,
        role: PeerRole,
        transcript: &AuthTranscript,
        signer: &S,
    ) -> Result<Self, AuthProofSignError<S::Error>>
    where
        S: AuthProofSigner,
    {
        let canonical = transcript
            .canonical_bytes()
            .map_err(AuthProofSignError::Transcript)?;
        let signature = signer
            .sign_auth_transcript(&canonical)
            .map_err(AuthProofSignError::Signer)?;

        Ok(Self {
            device_id: device_id.into(),
            role,
            signature,
        })
    }

    /// Verify this proof against the expected peer identity, role, and transcript.
    pub fn verify_transcript<V>(
        &self,
        expected_device_id: &str,
        expected_role: PeerRole,
        transcript: &AuthTranscript,
        verifier: &V,
    ) -> Result<(), AuthProofVerifyError<V::Error>>
    where
        V: AuthProofVerifier,
    {
        if self.device_id != expected_device_id {
            return Err(AuthProofVerifyError::DeviceIdMismatch {
                expected: expected_device_id.to_string(),
                actual: self.device_id.clone(),
            });
        }
        if self.role != expected_role {
            return Err(AuthProofVerifyError::ProofRoleMismatch {
                expected: expected_role,
                actual: self.role,
            });
        }
        if transcript.role != expected_role {
            return Err(AuthProofVerifyError::TranscriptRoleMismatch {
                expected: expected_role,
                actual: transcript.role,
            });
        }

        let canonical = transcript
            .canonical_bytes()
            .map_err(AuthProofVerifyError::Transcript)?;
        verifier
            .verify_auth_transcript(&canonical, &self.signature)
            .map_err(AuthProofVerifyError::Verifier)
    }
}

impl PeerRole {
    const fn canonical_byte(self) -> u8 {
        match self {
            Self::Initiator => 1,
            Self::Responder => 2,
        }
    }
}

/// Failure returned when a transcript cannot be represented in canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthTranscriptEncodeError {
    StringFieldTooLong {
        field: &'static str,
        max: usize,
        actual: usize,
    },
}

impl Display for AuthTranscriptEncodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StringFieldTooLong { field, max, actual } => write!(
                formatter,
                "auth transcript {field} is too long: max {max} bytes, got {actual} bytes"
            ),
        }
    }
}

impl Error for AuthTranscriptEncodeError {}

/// Failure returned while building an authentication proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthProofSignError<E> {
    Transcript(AuthTranscriptEncodeError),
    Signer(E),
}

impl<E> Display for AuthProofSignError<E>
where
    E: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transcript(error) => {
                write!(formatter, "failed to encode auth transcript: {error}")
            }
            Self::Signer(error) => write!(formatter, "failed to sign auth transcript: {error}"),
        }
    }
}

impl<E> Error for AuthProofSignError<E> where E: Error + 'static {}

/// Failure returned while verifying an authentication proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthProofVerifyError<E> {
    DeviceIdMismatch {
        expected: String,
        actual: String,
    },
    ProofRoleMismatch {
        expected: PeerRole,
        actual: PeerRole,
    },
    TranscriptRoleMismatch {
        expected: PeerRole,
        actual: PeerRole,
    },
    Transcript(AuthTranscriptEncodeError),
    Verifier(E),
}

impl<E> Display for AuthProofVerifyError<E>
where
    E: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceIdMismatch { expected, actual } => write!(
                formatter,
                "auth proof device id mismatch: expected {expected}, got {actual}"
            ),
            Self::ProofRoleMismatch { expected, actual } => write!(
                formatter,
                "auth proof role mismatch: expected {expected:?}, got {actual:?}"
            ),
            Self::TranscriptRoleMismatch { expected, actual } => write!(
                formatter,
                "auth transcript role mismatch: expected {expected:?}, got {actual:?}"
            ),
            Self::Transcript(error) => {
                write!(formatter, "failed to encode auth transcript: {error}")
            }
            Self::Verifier(error) => write!(formatter, "failed to verify auth transcript: {error}"),
        }
    }
}

impl<E> Error for AuthProofVerifyError<E> where E: Error + 'static {}

fn write_len_prefixed_str(
    output: &mut Vec<u8>,
    field: &'static str,
    value: &str,
) -> Result<(), AuthTranscriptEncodeError> {
    let bytes = value.as_bytes();
    if bytes.len() > AUTH_TRANSCRIPT_STRING_FIELD_MAX_LEN {
        return Err(AuthTranscriptEncodeError::StringFieldTooLong {
            field,
            max: AUTH_TRANSCRIPT_STRING_FIELD_MAX_LEN,
            actual: bytes.len(),
        });
    }

    write_u16(output, bytes.len() as u16);
    output.extend_from_slice(bytes);

    Ok(())
}

fn write_u8(output: &mut Vec<u8>, value: u8) {
    output.push(value);
}

fn write_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn write_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use std::fmt::{Display, Formatter, Write as _};

    use serde_json::json;

    use super::{
        AUTH_TRANSCRIPT_CANONICAL_VERSION, AUTH_TRANSCRIPT_LABEL, AuthProof, AuthProofSignError,
        AuthProofSigner, AuthProofVerifier, AuthProofVerifyError, AuthTranscript,
        AuthTranscriptEncodeError, CapabilityFlags, HANDSHAKE_NONCE_LEN, HandshakeMessage, Hello,
        PeerRole, ProtocolNegotiationError, ProtocolVersion, SESSION_TLS_EXPORTER_LABEL,
        SessionReady, TLS_EXPORTER_LEN,
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
            Ok(ProtocolVersion { major: 1, minor: 2 })
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
                "protocol": { "major": 1, "minor": 2 },
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

    #[test]
    fn auth_transcript_canonical_bytes_match_test_vector() {
        let transcript = auth_transcript_fixture(PeerRole::Initiator);

        let bytes = transcript
            .canonical_bytes()
            .expect("canonical transcript bytes");

        assert_eq!(bytes[0], AUTH_TRANSCRIPT_CANONICAL_VERSION);
        assert_eq!(
            hex(&bytes),
            concat!(
                "01",
                "000d616b72617a2d617574682d7631",
                "00086465766963652d61",
                "00086465766963652d62",
                "0101010101010101010101010101010101010101010101010101010101010101",
                "0202020202020202020202020202020202020202020202020202020202020202",
                "0001",
                "0002",
                "00000001",
                "00000003",
                "01",
                "0303030303030303030303030303030303030303030303030303030303030303"
            )
        );
    }

    #[test]
    fn auth_transcript_canonical_bytes_bind_peer_role() {
        let initiator = auth_transcript_fixture(PeerRole::Initiator)
            .canonical_bytes()
            .expect("initiator transcript");
        let responder = auth_transcript_fixture(PeerRole::Responder)
            .canonical_bytes()
            .expect("responder transcript");

        assert_ne!(initiator, responder);
        assert_eq!(initiator[initiator.len() - TLS_EXPORTER_LEN - 1], 1);
        assert_eq!(responder[responder.len() - TLS_EXPORTER_LEN - 1], 2);
    }

    #[test]
    fn auth_transcript_canonical_encoding_rejects_overlong_string_fields() {
        let transcript = AuthTranscript {
            local_device_id: "a".repeat(usize::from(u16::MAX) + 1),
            ..auth_transcript_fixture(PeerRole::Initiator)
        };

        assert_eq!(
            transcript.canonical_bytes(),
            Err(AuthTranscriptEncodeError::StringFieldTooLong {
                field: "local device id",
                max: usize::from(u16::MAX),
                actual: usize::from(u16::MAX) + 1,
            })
        );
    }

    #[test]
    fn auth_proof_sign_and_verify_round_trips_canonical_transcript() {
        let transcript = auth_transcript_fixture(PeerRole::Initiator);

        let proof =
            AuthProof::sign_transcript("device-a", PeerRole::Initiator, &transcript, &EchoSigner)
                .expect("auth proof");

        assert_eq!(proof.device_id, "device-a");
        assert_eq!(proof.role, PeerRole::Initiator);
        assert!(proof.signature.starts_with(b"akraz-test-signature:"));
        assert_eq!(
            proof.verify_transcript("device-a", PeerRole::Initiator, &transcript, &EchoVerifier),
            Ok(())
        );
    }

    #[test]
    fn auth_proof_signing_returns_transcript_encoding_failures() {
        let transcript = AuthTranscript {
            remote_device_id: "b".repeat(usize::from(u16::MAX) + 1),
            ..auth_transcript_fixture(PeerRole::Initiator)
        };

        assert_eq!(
            AuthProof::sign_transcript("device-a", PeerRole::Initiator, &transcript, &EchoSigner),
            Err(AuthProofSignError::Transcript(
                AuthTranscriptEncodeError::StringFieldTooLong {
                    field: "remote device id",
                    max: usize::from(u16::MAX),
                    actual: usize::from(u16::MAX) + 1,
                }
            ))
        );
    }

    #[test]
    fn auth_proof_verification_rejects_mismatched_identity_before_signature_check() {
        let transcript = auth_transcript_fixture(PeerRole::Initiator);
        let proof =
            AuthProof::sign_transcript("device-a", PeerRole::Initiator, &transcript, &EchoSigner)
                .expect("auth proof");

        assert_eq!(
            proof.verify_transcript("device-b", PeerRole::Initiator, &transcript, &EchoVerifier),
            Err(AuthProofVerifyError::DeviceIdMismatch {
                expected: "device-b".to_string(),
                actual: "device-a".to_string(),
            })
        );
        assert_eq!(
            proof.verify_transcript("device-a", PeerRole::Responder, &transcript, &EchoVerifier),
            Err(AuthProofVerifyError::ProofRoleMismatch {
                expected: PeerRole::Responder,
                actual: PeerRole::Initiator,
            })
        );
    }

    #[test]
    fn auth_proof_verification_rejects_invalid_signature() {
        let transcript = auth_transcript_fixture(PeerRole::Initiator);
        let mut proof =
            AuthProof::sign_transcript("device-a", PeerRole::Initiator, &transcript, &EchoSigner)
                .expect("auth proof");
        proof.signature.push(0);

        assert_eq!(
            proof.verify_transcript("device-a", PeerRole::Initiator, &transcript, &EchoVerifier),
            Err(AuthProofVerifyError::Verifier(TestCryptoError::Rejected))
        );
    }

    fn auth_transcript_fixture(role: PeerRole) -> AuthTranscript {
        AuthTranscript {
            local_device_id: "device-a".to_string(),
            remote_device_id: "device-b".to_string(),
            local_nonce: [1; HANDSHAKE_NONCE_LEN],
            remote_nonce: [2; HANDSHAKE_NONCE_LEN],
            protocol: ProtocolVersion::CURRENT,
            local_capabilities: CapabilityFlags::POINTER,
            remote_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            role,
            tls_exporter: [3; TLS_EXPORTER_LEN],
        }
    }

    fn hex(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            if let Err(error) = write!(&mut output, "{byte:02x}") {
                panic!("failed to write hex test vector: {error}");
            }
        }

        output
    }

    struct EchoSigner;

    impl AuthProofSigner for EchoSigner {
        type Error = TestCryptoError;

        fn sign_auth_transcript(
            &self,
            canonical_transcript: &[u8],
        ) -> Result<Vec<u8>, Self::Error> {
            Ok(test_signature(canonical_transcript))
        }
    }

    struct EchoVerifier;

    impl AuthProofVerifier for EchoVerifier {
        type Error = TestCryptoError;

        fn verify_auth_transcript(
            &self,
            canonical_transcript: &[u8],
            signature: &[u8],
        ) -> Result<(), Self::Error> {
            if signature == test_signature(canonical_transcript) {
                Ok(())
            } else {
                Err(TestCryptoError::Rejected)
            }
        }
    }

    fn test_signature(canonical_transcript: &[u8]) -> Vec<u8> {
        let mut signature = b"akraz-test-signature:".to_vec();
        signature.extend_from_slice(canonical_transcript);
        signature
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestCryptoError {
        Rejected,
    }

    impl Display for TestCryptoError {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Rejected => formatter.write_str("signature rejected"),
            }
        }
    }
}
