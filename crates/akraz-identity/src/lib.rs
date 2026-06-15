//! Identity-key adapter boundary for akraz peer authentication.

use akraz_protocol::{
    AuthProof, AuthProofSignError, AuthProofSigner, AuthProofVerifier, AuthProofVerifyError,
    AuthTranscript, CapabilityFlags, PeerRole,
};

/// Local device identity metadata paired with a long-lived identity public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    device_id: String,
    display_name: String,
    identity_public_key: Vec<u8>,
    fingerprint: String,
}

impl DeviceIdentity {
    /// Create local device identity metadata.
    pub fn new(
        device_id: impl Into<String>,
        display_name: impl Into<String>,
        identity_public_key: impl Into<Vec<u8>>,
        fingerprint: impl Into<String>,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            display_name: display_name.into(),
            identity_public_key: identity_public_key.into(),
            fingerprint: fingerprint.into(),
        }
    }

    /// Stable local device id.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Public key bytes used by peers to verify this device.
    pub fn identity_public_key(&self) -> &[u8] {
        &self.identity_public_key
    }

    /// Short user-visible fingerprint derived from the identity public key.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Trusted peer identity metadata saved after pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeerIdentity {
    peer_id: String,
    display_name: String,
    identity_public_key: Vec<u8>,
    fingerprint: String,
    capabilities: CapabilityFlags,
}

impl TrustedPeerIdentity {
    /// Create trusted peer identity metadata.
    pub fn new(
        peer_id: impl Into<String>,
        display_name: impl Into<String>,
        identity_public_key: impl Into<Vec<u8>>,
        fingerprint: impl Into<String>,
        capabilities: CapabilityFlags,
    ) -> Self {
        Self {
            peer_id: peer_id.into(),
            display_name: display_name.into(),
            identity_public_key: identity_public_key.into(),
            fingerprint: fingerprint.into(),
            capabilities,
        }
    }

    /// Stable peer id.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Public key bytes trusted for this peer.
    pub fn identity_public_key(&self) -> &[u8] {
        &self.identity_public_key
    }

    /// Short user-visible fingerprint derived from the identity public key.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Capabilities observed when the peer was paired or last refreshed.
    pub fn capabilities(&self) -> CapabilityFlags {
        self.capabilities
    }
}

/// Secret-key operation used to sign canonical auth transcripts.
pub trait IdentitySecretKey {
    type Error;

    fn sign_identity(&self, canonical_transcript: &[u8]) -> Result<Vec<u8>, Self::Error>;
}

/// Public-key operation used to verify canonical auth transcripts.
pub trait IdentityPublicKey {
    type Error;

    fn verify_identity(
        &self,
        canonical_transcript: &[u8],
        signature: &[u8],
    ) -> Result<(), Self::Error>;
}

/// Local identity plus the signing implementation that owns or reaches the secret key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIdentity<S> {
    identity: DeviceIdentity,
    secret_key: S,
}

impl<S> LocalIdentity<S> {
    /// Create a local signing adapter.
    pub fn new(identity: DeviceIdentity, secret_key: S) -> Self {
        Self {
            identity,
            secret_key,
        }
    }

    /// Return the local identity metadata.
    pub fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    /// Decompose this adapter into metadata and the signing implementation.
    pub fn into_parts(self) -> (DeviceIdentity, S) {
        (self.identity, self.secret_key)
    }
}

impl<S> LocalIdentity<S>
where
    S: IdentitySecretKey,
{
    /// Build an authentication proof for this local identity.
    pub fn sign_auth_proof(
        &self,
        role: PeerRole,
        transcript: &AuthTranscript,
    ) -> Result<AuthProof, AuthProofSignError<S::Error>> {
        AuthProof::sign_transcript(self.identity.device_id(), role, transcript, self)
    }
}

impl<S> AuthProofSigner for LocalIdentity<S>
where
    S: IdentitySecretKey,
{
    type Error = S::Error;

    fn sign_auth_transcript(&self, canonical_transcript: &[u8]) -> Result<Vec<u8>, Self::Error> {
        self.secret_key.sign_identity(canonical_transcript)
    }
}

/// Trusted peer identity plus the verifier implementation for its public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeer<V> {
    identity: TrustedPeerIdentity,
    public_key: V,
}

impl<V> TrustedPeer<V> {
    /// Create a trusted peer verification adapter.
    pub fn new(identity: TrustedPeerIdentity, public_key: V) -> Self {
        Self {
            identity,
            public_key,
        }
    }

    /// Return the trusted peer metadata.
    pub fn identity(&self) -> &TrustedPeerIdentity {
        &self.identity
    }

    /// Decompose this adapter into metadata and the verification implementation.
    pub fn into_parts(self) -> (TrustedPeerIdentity, V) {
        (self.identity, self.public_key)
    }
}

impl<V> TrustedPeer<V>
where
    V: IdentityPublicKey,
{
    /// Verify an authentication proof for this trusted peer.
    pub fn verify_auth_proof(
        &self,
        expected_role: PeerRole,
        transcript: &AuthTranscript,
        proof: &AuthProof,
    ) -> Result<(), AuthProofVerifyError<V::Error>> {
        proof.verify_transcript(self.identity.peer_id(), expected_role, transcript, self)
    }
}

impl<V> AuthProofVerifier for TrustedPeer<V>
where
    V: IdentityPublicKey,
{
    type Error = V::Error;

    fn verify_auth_transcript(
        &self,
        canonical_transcript: &[u8],
        signature: &[u8],
    ) -> Result<(), Self::Error> {
        self.public_key
            .verify_identity(canonical_transcript, signature)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::{Display, Formatter};

    use akraz_protocol::{
        AuthProofVerifyError, AuthTranscript, CapabilityFlags, HANDSHAKE_NONCE_LEN, PeerRole,
        ProtocolVersion, TLS_EXPORTER_LEN,
    };

    use super::{
        DeviceIdentity, IdentityPublicKey, IdentitySecretKey, LocalIdentity, TrustedPeer,
        TrustedPeerIdentity,
    };

    #[test]
    fn local_identity_signs_auth_proof_with_its_device_id() {
        let identity =
            DeviceIdentity::new("device-a", "Device A", b"public-a".to_vec(), "AKRZ-TEST-A");
        let local = LocalIdentity::new(identity, EchoSecretKey::new(b"public-a"));
        let transcript = auth_transcript_fixture(PeerRole::Initiator);

        let proof = local
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .expect("auth proof");

        assert_eq!(proof.device_id, "device-a");
        assert_eq!(proof.role, PeerRole::Initiator);
        assert!(proof.signature.starts_with(b"akraz-identity-test:"));
    }

    #[test]
    fn trusted_peer_verifies_matching_auth_proof() {
        let identity = TrustedPeerIdentity::new(
            "device-a",
            "Device A",
            b"public-a".to_vec(),
            "AKRZ-TEST-A",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let trusted = TrustedPeer::new(identity, EchoPublicKey::new(b"public-a"));
        let local = LocalIdentity::new(
            DeviceIdentity::new("device-a", "Device A", b"public-a".to_vec(), "AKRZ-TEST-A"),
            EchoSecretKey::new(b"public-a"),
        );
        let transcript = auth_transcript_fixture(PeerRole::Initiator);
        let proof = local
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .expect("auth proof");

        assert_eq!(
            trusted.verify_auth_proof(PeerRole::Initiator, &transcript, &proof),
            Ok(())
        );
    }

    #[test]
    fn trusted_peer_rejects_untrusted_device_id_before_crypto_check() {
        let identity = TrustedPeerIdentity::new(
            "device-b",
            "Device B",
            b"public-b".to_vec(),
            "AKRZ-TEST-B",
            CapabilityFlags::POINTER,
        );
        let trusted = TrustedPeer::new(identity, EchoPublicKey::new(b"public-a"));
        let local = LocalIdentity::new(
            DeviceIdentity::new("device-a", "Device A", b"public-a".to_vec(), "AKRZ-TEST-A"),
            EchoSecretKey::new(b"public-a"),
        );
        let transcript = auth_transcript_fixture(PeerRole::Initiator);
        let proof = local
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .expect("auth proof");

        assert_eq!(
            trusted.verify_auth_proof(PeerRole::Initiator, &transcript, &proof),
            Err(AuthProofVerifyError::DeviceIdMismatch {
                expected: "device-b".to_string(),
                actual: "device-a".to_string(),
            })
        );
    }

    #[test]
    fn trusted_peer_rejects_invalid_signature() {
        let identity = TrustedPeerIdentity::new(
            "device-a",
            "Device A",
            b"public-a".to_vec(),
            "AKRZ-TEST-A",
            CapabilityFlags::POINTER,
        );
        let trusted = TrustedPeer::new(identity, EchoPublicKey::new(b"public-b"));
        let local = LocalIdentity::new(
            DeviceIdentity::new("device-a", "Device A", b"public-a".to_vec(), "AKRZ-TEST-A"),
            EchoSecretKey::new(b"public-a"),
        );
        let transcript = auth_transcript_fixture(PeerRole::Initiator);
        let proof = local
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .expect("auth proof");

        assert_eq!(
            trusted.verify_auth_proof(PeerRole::Initiator, &transcript, &proof),
            Err(AuthProofVerifyError::Verifier(TestIdentityError::Rejected))
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct EchoSecretKey {
        public_key: Vec<u8>,
    }

    impl EchoSecretKey {
        fn new(public_key: &[u8]) -> Self {
            Self {
                public_key: public_key.to_vec(),
            }
        }
    }

    impl IdentitySecretKey for EchoSecretKey {
        type Error = TestIdentityError;

        fn sign_identity(&self, canonical_transcript: &[u8]) -> Result<Vec<u8>, Self::Error> {
            let mut signature = b"akraz-identity-test:".to_vec();
            signature.extend_from_slice(&self.public_key);
            signature.push(b':');
            signature.extend_from_slice(canonical_transcript);
            Ok(signature)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct EchoPublicKey {
        public_key: Vec<u8>,
    }

    impl EchoPublicKey {
        fn new(public_key: &[u8]) -> Self {
            Self {
                public_key: public_key.to_vec(),
            }
        }
    }

    impl IdentityPublicKey for EchoPublicKey {
        type Error = TestIdentityError;

        fn verify_identity(
            &self,
            canonical_transcript: &[u8],
            signature: &[u8],
        ) -> Result<(), Self::Error> {
            let mut expected = b"akraz-identity-test:".to_vec();
            expected.extend_from_slice(&self.public_key);
            expected.push(b':');
            expected.extend_from_slice(canonical_transcript);
            if signature == expected {
                Ok(())
            } else {
                Err(TestIdentityError::Rejected)
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestIdentityError {
        Rejected,
    }

    impl Display for TestIdentityError {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Rejected => formatter.write_str("identity verification rejected"),
            }
        }
    }
}
