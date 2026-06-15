//! Identity-key adapter boundary for akraz peer authentication.

use std::{
    convert::Infallible,
    error::Error,
    fmt::{Debug, Display, Formatter},
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use akraz_protocol::{
    AuthProof, AuthProofSignError, AuthProofSigner, AuthProofVerifier, AuthProofVerifyError,
    AuthTranscript, CapabilityFlags, PeerRole,
};
use data_encoding::{BASE32_NOPAD, BASE64};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const IDENTITY_STORE_VERSION: u32 = 1;
const ED25519_SECRET_KEY_LEN: usize = 32;
const ED25519_PUBLIC_KEY_LEN: usize = 32;
const FINGERPRINT_BODY_LEN: usize = 32;
const FINGERPRINT_GROUP_LEN: usize = 4;
const DEFAULT_DISPLAY_NAME: &str = "Akraz Device";

/// Build the stable Akraz fingerprint shown to users when they compare devices.
pub fn fingerprint_for_public_key(identity_public_key: &[u8]) -> String {
    let digest = blake3::hash(identity_public_key);
    let encoded = BASE32_NOPAD.encode(digest.as_bytes());
    let body = &encoded[..FINGERPRINT_BODY_LEN];
    let mut fingerprint = String::from("AKRZ");
    for chunk in body.as_bytes().chunks(FINGERPRINT_GROUP_LEN) {
        fingerprint.push('-');
        for byte in chunk {
            fingerprint.push(char::from(*byte));
        }
    }
    fingerprint
}

/// Ed25519 signing key used as the local long-lived Akraz identity key.
pub struct Ed25519IdentityKey {
    signing_key: SigningKey,
}

impl Ed25519IdentityKey {
    /// Generate a new identity key from OS-backed secure randomness.
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// Rebuild an identity key from raw secret bytes loaded by a storage adapter.
    pub fn from_secret_key_bytes(secret_key: [u8; ED25519_SECRET_KEY_LEN]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&secret_key),
        }
    }

    /// Export raw secret bytes for storage adapters.
    ///
    /// This is sensitive key material. It exists so OS vault and fallback file adapters can persist
    /// the key; callers must not log or expose the returned bytes.
    pub fn export_secret_key_bytes(&self) -> [u8; ED25519_SECRET_KEY_LEN] {
        self.signing_key.to_bytes()
    }

    /// Return the public key bytes derived from this identity key.
    pub fn public_key_bytes(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Return the user-visible fingerprint for this key.
    pub fn fingerprint(&self) -> String {
        fingerprint_for_public_key(&self.public_key_bytes())
    }
}

impl Debug for Ed25519IdentityKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ed25519IdentityKey")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl IdentitySecretKey for Ed25519IdentityKey {
    type Error = Infallible;

    fn sign_identity(&self, canonical_transcript: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Ok(self
            .signing_key
            .sign(canonical_transcript)
            .to_bytes()
            .to_vec())
    }
}

/// Ed25519 public key trusted for a paired peer.
#[derive(Clone, PartialEq, Eq)]
pub struct Ed25519PublicKey {
    verifying_key: VerifyingKey,
}

impl Ed25519PublicKey {
    /// Decode trusted peer public key bytes.
    pub fn from_public_key_bytes(bytes: &[u8]) -> Result<Self, IdentityKeyError> {
        let bytes = fixed_array::<ED25519_PUBLIC_KEY_LEN>(bytes)
            .map_err(|actual| IdentityKeyError::InvalidPublicKeyLength { actual })?;
        let verifying_key =
            VerifyingKey::from_bytes(&bytes).map_err(IdentityKeyError::InvalidPublicKey)?;
        Ok(Self { verifying_key })
    }

    /// Return this trusted public key as raw bytes.
    pub fn public_key_bytes(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
        self.verifying_key.to_bytes()
    }

    /// Return the user-visible fingerprint for this public key.
    pub fn fingerprint(&self) -> String {
        fingerprint_for_public_key(&self.public_key_bytes())
    }
}

impl Debug for Ed25519PublicKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ed25519PublicKey")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl IdentityPublicKey for Ed25519PublicKey {
    type Error = IdentityKeyError;

    fn verify_identity(
        &self,
        canonical_transcript: &[u8],
        signature: &[u8],
    ) -> Result<(), Self::Error> {
        let signature = Signature::try_from(signature).map_err(|_| {
            IdentityKeyError::InvalidSignatureLength {
                actual: signature.len(),
            }
        })?;
        self.verifying_key
            .verify(canonical_transcript, &signature)
            .map_err(IdentityKeyError::InvalidSignature)
    }
}

/// Errors produced by Ed25519 identity key operations.
#[derive(Debug)]
pub enum IdentityKeyError {
    InvalidPublicKeyLength { actual: usize },
    InvalidPublicKey(ed25519_dalek::SignatureError),
    InvalidSignatureLength { actual: usize },
    InvalidSignature(ed25519_dalek::SignatureError),
}

impl Display for IdentityKeyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPublicKeyLength { actual } => {
                write!(formatter, "invalid Ed25519 public key length: {actual}")
            }
            Self::InvalidPublicKey(_) => formatter.write_str("invalid Ed25519 public key"),
            Self::InvalidSignatureLength { actual } => {
                write!(formatter, "invalid Ed25519 signature length: {actual}")
            }
            Self::InvalidSignature(_) => formatter.write_str("invalid Ed25519 signature"),
        }
    }
}

impl Error for IdentityKeyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPublicKey(source) | Self::InvalidSignature(source) => Some(source),
            Self::InvalidPublicKeyLength { .. } | Self::InvalidSignatureLength { .. } => None,
        }
    }
}

/// Local identity loaded from durable storage.
#[derive(Debug)]
pub struct StoredLocalIdentity {
    identity: DeviceIdentity,
    secret_key: Ed25519IdentityKey,
}

impl StoredLocalIdentity {
    /// Create a stored identity value from verified identity metadata and a secret key.
    pub fn new(identity: DeviceIdentity, secret_key: Ed25519IdentityKey) -> Self {
        Self {
            identity,
            secret_key,
        }
    }

    /// Return local identity metadata.
    pub fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    /// Return the local signing key.
    pub fn secret_key(&self) -> &Ed25519IdentityKey {
        &self.secret_key
    }

    /// Convert this stored identity into the signing adapter used by auth proof code.
    pub fn into_local_identity(self) -> LocalIdentity<Ed25519IdentityKey> {
        LocalIdentity::new(self.identity, self.secret_key)
    }
}

/// Fallback file-backed identity-key store.
///
/// This is the portable fallback behind OS vault adapters. It rejects malformed files instead of
/// silently replacing them so identity changes cannot be hidden by a parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentityStore {
    path: PathBuf,
}

impl FileIdentityStore {
    /// Create a file-backed identity store at the exact file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Return the configured identity file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the existing identity or create one on first run.
    pub fn load_or_create(
        &self,
        display_name: impl AsRef<str>,
    ) -> Result<StoredLocalIdentity, IdentityStoreError> {
        let display_name = normalize_display_name(display_name.as_ref());
        match self.load_existing() {
            Ok(identity) => Ok(identity),
            Err(IdentityStoreError::MissingIdentityFile { .. }) => {
                self.create_new_identity(display_name)
            }
            Err(error) => Err(error),
        }
    }

    fn load_existing(&self) -> Result<StoredLocalIdentity, IdentityStoreError> {
        reject_identity_path_symlink(&self.path)?;
        let contents = fs::read_to_string(&self.path).map_err(|source| match source.kind() {
            ErrorKind::NotFound => IdentityStoreError::MissingIdentityFile {
                path: self.path.clone(),
            },
            _ => IdentityStoreError::ReadIdentityFile {
                path: self.path.clone(),
                source,
            },
        })?;
        let stored: StoredIdentityFile = serde_json::from_str(&contents).map_err(|source| {
            IdentityStoreError::CorruptIdentityFile {
                path: self.path.clone(),
                source: CorruptIdentityFileSource::Json(source),
            }
        })?;
        stored.into_stored_identity(&self.path)
    }

    fn create_new_identity(
        &self,
        display_name: String,
    ) -> Result<StoredLocalIdentity, IdentityStoreError> {
        reject_identity_path_symlink(&self.path)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                IdentityStoreError::CreateStoreDirectory {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }

        let secret_key = Ed25519IdentityKey::generate();
        let public_key = secret_key.public_key_bytes();
        let identity = DeviceIdentity::new(
            Uuid::new_v4().to_string(),
            display_name,
            public_key,
            fingerprint_for_public_key(&public_key),
        );
        let stored = StoredIdentityFile::from_identity(&identity, &secret_key);
        let mut contents = serde_json::to_vec_pretty(&stored).map_err(|source| {
            IdentityStoreError::EncodeIdentityFile {
                path: self.path.clone(),
                source,
            }
        })?;
        contents.push(b'\n');

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        set_secret_file_create_mode(&mut options);
        let mut file = match options.open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                return self.load_existing();
            }
            Err(source) => {
                return Err(IdentityStoreError::WriteIdentityFile {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        file.write_all(&contents)
            .map_err(|source| IdentityStoreError::WriteIdentityFile {
                path: self.path.clone(),
                source,
            })?;

        Ok(StoredLocalIdentity::new(identity, secret_key))
    }
}

/// Errors produced while loading or creating durable identity key material.
#[derive(Debug)]
pub enum IdentityStoreError {
    MissingIdentityFile {
        path: PathBuf,
    },
    IdentityPathIsSymlink {
        path: PathBuf,
    },
    ReadIdentityMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadIdentityFile {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateStoreDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    WriteIdentityFile {
        path: PathBuf,
        source: std::io::Error,
    },
    EncodeIdentityFile {
        path: PathBuf,
        source: serde_json::Error,
    },
    CorruptIdentityFile {
        path: PathBuf,
        source: CorruptIdentityFileSource,
    },
    UnsupportedIdentityFileVersion {
        path: PathBuf,
        version: u32,
    },
}

impl Display for IdentityStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingIdentityFile { path } => {
                write!(
                    formatter,
                    "identity file does not exist: {}",
                    path.display()
                )
            }
            Self::IdentityPathIsSymlink { path } => {
                write!(
                    formatter,
                    "identity file path is a symlink: {}",
                    path.display()
                )
            }
            Self::ReadIdentityMetadata { path, .. } => {
                write!(
                    formatter,
                    "failed to read identity file metadata: {}",
                    path.display()
                )
            }
            Self::ReadIdentityFile { path, .. } => {
                write!(
                    formatter,
                    "failed to read identity file: {}",
                    path.display()
                )
            }
            Self::CreateStoreDirectory { path, .. } => {
                write!(
                    formatter,
                    "failed to create identity store directory: {}",
                    path.display()
                )
            }
            Self::WriteIdentityFile { path, .. } => {
                write!(
                    formatter,
                    "failed to write identity file: {}",
                    path.display()
                )
            }
            Self::EncodeIdentityFile { path, .. } => {
                write!(
                    formatter,
                    "failed to encode identity file: {}",
                    path.display()
                )
            }
            Self::CorruptIdentityFile { path, source } => {
                write!(
                    formatter,
                    "corrupt identity file at {}: {source}",
                    path.display()
                )
            }
            Self::UnsupportedIdentityFileVersion { path, version } => {
                write!(
                    formatter,
                    "unsupported identity file version {version} at {}",
                    path.display()
                )
            }
        }
    }
}

impl Error for IdentityStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadIdentityMetadata { source, .. }
            | Self::ReadIdentityFile { source, .. }
            | Self::CreateStoreDirectory { source, .. }
            | Self::WriteIdentityFile { source, .. } => Some(source),
            Self::EncodeIdentityFile { source, .. } => Some(source),
            Self::CorruptIdentityFile { source, .. } => Some(source),
            Self::MissingIdentityFile { .. }
            | Self::IdentityPathIsSymlink { .. }
            | Self::UnsupportedIdentityFileVersion { .. } => None,
        }
    }
}

/// Detailed reason an identity file could not be trusted.
#[derive(Debug)]
pub enum CorruptIdentityFileSource {
    Json(serde_json::Error),
    InvalidDeviceId(uuid::Error),
    InvalidSecretKeyEncoding(data_encoding::DecodeError),
    InvalidPublicKeyEncoding(data_encoding::DecodeError),
    InvalidSecretKeyLength { actual: usize },
    InvalidPublicKeyLength { actual: usize },
    PublicKeyMismatch,
    FingerprintMismatch { expected: String, actual: String },
}

impl Display for CorruptIdentityFileSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(_) => formatter.write_str("invalid JSON"),
            Self::InvalidDeviceId(_) => formatter.write_str("invalid device id"),
            Self::InvalidSecretKeyEncoding(_) => formatter.write_str("invalid secret key encoding"),
            Self::InvalidPublicKeyEncoding(_) => formatter.write_str("invalid public key encoding"),
            Self::InvalidSecretKeyLength { actual } => {
                write!(formatter, "invalid secret key length: {actual}")
            }
            Self::InvalidPublicKeyLength { actual } => {
                write!(formatter, "invalid public key length: {actual}")
            }
            Self::PublicKeyMismatch => {
                formatter.write_str("stored public key does not match secret key")
            }
            Self::FingerprintMismatch { expected, actual } => {
                write!(
                    formatter,
                    "stored fingerprint mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl Error for CorruptIdentityFileSource {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(source) => Some(source),
            Self::InvalidDeviceId(source) => Some(source),
            Self::InvalidSecretKeyEncoding(source) | Self::InvalidPublicKeyEncoding(source) => {
                Some(source)
            }
            Self::InvalidSecretKeyLength { .. }
            | Self::InvalidPublicKeyLength { .. }
            | Self::PublicKeyMismatch
            | Self::FingerprintMismatch { .. } => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredIdentityFile {
    version: u32,
    device_id: String,
    display_name: String,
    identity_secret_key: String,
    identity_public_key: String,
    fingerprint: String,
}

impl StoredIdentityFile {
    fn from_identity(identity: &DeviceIdentity, secret_key: &Ed25519IdentityKey) -> Self {
        Self {
            version: IDENTITY_STORE_VERSION,
            device_id: identity.device_id().to_string(),
            display_name: identity.display_name().to_string(),
            identity_secret_key: BASE64.encode(&secret_key.export_secret_key_bytes()),
            identity_public_key: BASE64.encode(identity.identity_public_key()),
            fingerprint: identity.fingerprint().to_string(),
        }
    }

    fn into_stored_identity(self, path: &Path) -> Result<StoredLocalIdentity, IdentityStoreError> {
        if self.version != IDENTITY_STORE_VERSION {
            return Err(IdentityStoreError::UnsupportedIdentityFileVersion {
                path: path.to_path_buf(),
                version: self.version,
            });
        }
        Uuid::parse_str(&self.device_id).map_err(|source| {
            IdentityStoreError::CorruptIdentityFile {
                path: path.to_path_buf(),
                source: CorruptIdentityFileSource::InvalidDeviceId(source),
            }
        })?;
        let secret_key = decode_fixed_base64::<ED25519_SECRET_KEY_LEN>(
            &self.identity_secret_key,
            path,
            CorruptIdentityFileSource::InvalidSecretKeyEncoding,
            |actual| CorruptIdentityFileSource::InvalidSecretKeyLength { actual },
        )?;
        let stored_public_key = decode_fixed_base64::<ED25519_PUBLIC_KEY_LEN>(
            &self.identity_public_key,
            path,
            CorruptIdentityFileSource::InvalidPublicKeyEncoding,
            |actual| CorruptIdentityFileSource::InvalidPublicKeyLength { actual },
        )?;
        let secret_key = Ed25519IdentityKey::from_secret_key_bytes(secret_key);
        let derived_public_key = secret_key.public_key_bytes();
        if stored_public_key != derived_public_key {
            return Err(IdentityStoreError::CorruptIdentityFile {
                path: path.to_path_buf(),
                source: CorruptIdentityFileSource::PublicKeyMismatch,
            });
        }
        let expected_fingerprint = fingerprint_for_public_key(&derived_public_key);
        if self.fingerprint != expected_fingerprint {
            return Err(IdentityStoreError::CorruptIdentityFile {
                path: path.to_path_buf(),
                source: CorruptIdentityFileSource::FingerprintMismatch {
                    expected: expected_fingerprint,
                    actual: self.fingerprint,
                },
            });
        }
        let identity = DeviceIdentity::new(
            self.device_id,
            normalize_display_name(&self.display_name),
            derived_public_key,
            expected_fingerprint,
        );
        Ok(StoredLocalIdentity::new(identity, secret_key))
    }
}

fn normalize_display_name(display_name: &str) -> String {
    let display_name = display_name.trim();
    if display_name.is_empty() {
        DEFAULT_DISPLAY_NAME.to_string()
    } else {
        display_name.to_string()
    }
}

fn reject_identity_path_symlink(path: &Path) -> Result<(), IdentityStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(IdentityStoreError::IdentityPathIsSymlink {
                path: path.to_path_buf(),
            })
        }
        Ok(_) => Ok(()),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(IdentityStoreError::ReadIdentityMetadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn decode_fixed_base64<const N: usize>(
    encoded: &str,
    path: &Path,
    decode_error: fn(data_encoding::DecodeError) -> CorruptIdentityFileSource,
    length_error: fn(usize) -> CorruptIdentityFileSource,
) -> Result<[u8; N], IdentityStoreError> {
    let decoded = BASE64.decode(encoded.as_bytes()).map_err(|source| {
        IdentityStoreError::CorruptIdentityFile {
            path: path.to_path_buf(),
            source: decode_error(source),
        }
    })?;
    fixed_array::<N>(&decoded).map_err(|actual| IdentityStoreError::CorruptIdentityFile {
        path: path.to_path_buf(),
        source: length_error(actual),
    })
}

fn fixed_array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], usize> {
    bytes.try_into().map_err(|_| bytes.len())
}

#[cfg(unix)]
fn set_secret_file_create_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_secret_file_create_mode(_options: &mut OpenOptions) {}

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
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use data_encoding::BASE64;
    use serde_json::Value;
    use uuid::Uuid;

    use akraz_protocol::{
        AuthProofVerifyError, AuthTranscript, CapabilityFlags, HANDSHAKE_NONCE_LEN, PeerRole,
        ProtocolVersion, TLS_EXPORTER_LEN,
    };

    use super::{
        CorruptIdentityFileSource, DeviceIdentity, Ed25519IdentityKey, Ed25519PublicKey,
        FileIdentityStore, IdentityPublicKey, IdentitySecretKey, IdentityStoreError, LocalIdentity,
        TrustedPeer, TrustedPeerIdentity, fingerprint_for_public_key,
    };

    #[test]
    fn ed25519_identity_key_signs_and_verifies_auth_proof() {
        let secret_key = Ed25519IdentityKey::generate();
        let public_key = Ed25519PublicKey::from_public_key_bytes(&secret_key.public_key_bytes())
            .expect("public key");
        let fingerprint = fingerprint_for_public_key(&secret_key.public_key_bytes());
        let local = LocalIdentity::new(
            DeviceIdentity::new(
                "device-a",
                "Device A",
                secret_key.public_key_bytes(),
                fingerprint.clone(),
            ),
            secret_key,
        );
        let trusted = TrustedPeer::new(
            TrustedPeerIdentity::new(
                "device-a",
                "Device A",
                public_key.public_key_bytes(),
                fingerprint,
                CapabilityFlags::POINTER,
            ),
            public_key,
        );
        let transcript = auth_transcript_fixture(PeerRole::Initiator);

        let proof = local
            .sign_auth_proof(PeerRole::Initiator, &transcript)
            .expect("auth proof");

        assert!(
            trusted
                .verify_auth_proof(PeerRole::Initiator, &transcript, &proof)
                .is_ok()
        );
    }

    #[test]
    fn file_identity_store_creates_then_reuses_existing_identity() {
        let path = unique_identity_path("reuse");
        let store = FileIdentityStore::new(&path);

        let created = store.load_or_create("Device A").expect("created identity");
        let created_device_id = created.identity().device_id().to_string();
        let created_secret = created.secret_key().export_secret_key_bytes();

        Uuid::parse_str(&created_device_id).expect("device id is UUID");
        assert_eq!(created.identity().display_name(), "Device A");
        assert_eq!(created.identity().identity_public_key().len(), 32);
        assert_eq!(
            created.identity().fingerprint(),
            fingerprint_for_public_key(created.identity().identity_public_key())
        );

        let loaded = store
            .load_or_create("Changed Name")
            .expect("loaded identity");

        assert_eq!(loaded.identity().device_id(), created_device_id);
        assert_eq!(loaded.identity().display_name(), "Device A");
        assert_eq!(
            loaded.identity().identity_public_key(),
            created.identity().identity_public_key()
        );
        assert_eq!(
            loaded.secret_key().export_secret_key_bytes(),
            created_secret
        );

        remove_identity_path(path);
    }

    #[test]
    fn file_identity_store_rejects_corrupt_file_without_replacing_it() {
        let path = unique_identity_path("corrupt-json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, "not json").expect("write corrupt file");
        let store = FileIdentityStore::new(&path);

        let error = store
            .load_or_create("Device A")
            .expect_err("corrupt file should fail closed");

        assert!(matches!(
            error,
            IdentityStoreError::CorruptIdentityFile {
                source: CorruptIdentityFileSource::Json(_),
                ..
            }
        ));
        assert_eq!(
            fs::read_to_string(&path).expect("read corrupt file"),
            "not json"
        );

        remove_identity_path(path);
    }

    #[test]
    fn file_identity_store_rejects_mismatched_public_key() {
        let path = unique_identity_path("mismatched-public-key");
        let store = FileIdentityStore::new(&path);
        let created = store.load_or_create("Device A").expect("created identity");
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("stored identity JSON"))
                .expect("stored identity value");
        value["identityPublicKey"] = Value::String(BASE64.encode(&[9; 32]));
        fs::write(
            &path,
            serde_json::to_string_pretty(&value).expect("tampered JSON"),
        )
        .expect("write tampered file");

        let error = store
            .load_or_create("Device A")
            .expect_err("mismatched public key should fail closed");

        assert!(matches!(
            error,
            IdentityStoreError::CorruptIdentityFile {
                source: CorruptIdentityFileSource::PublicKeyMismatch,
                ..
            }
        ));
        assert_ne!(
            BASE64.encode(created.identity().identity_public_key()),
            BASE64.encode(&[9; 32])
        );

        remove_identity_path(path);
    }

    #[test]
    fn ed25519_identity_key_debug_omits_secret_key_material() {
        let secret_key = Ed25519IdentityKey::generate();
        let secret_key_base64 = BASE64.encode(&secret_key.export_secret_key_bytes());
        let debug = format!("{secret_key:?}");

        assert!(!debug.contains(&secret_key_base64));
        assert!(debug.contains(&secret_key.fingerprint()));
    }

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

    impl std::fmt::Display for TestIdentityError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Rejected => formatter.write_str("identity verification rejected"),
            }
        }
    }

    fn unique_identity_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir()
            .join("akraz-identity-tests")
            .join(format!("{label}-{}-{nanos}", std::process::id()))
            .join("identity.json")
    }

    fn remove_identity_path(path: PathBuf) {
        let _ = fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}
