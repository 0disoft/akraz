//! Discovery contracts for local-network Akraz peers.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akraz_identity::{DeviceIdentity, PairingIdentityDocument, TrustedPeerIdentity};
use akraz_protocol::CapabilityFlags;
use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

/// DNS-SD service type used by Akraz mDNS discovery.
pub const AKRAZ_DISCOVERY_SERVICE_TYPE: &str = "_akraz._tcp.local.";

/// Current Akraz discovery TXT contract version.
pub const AKRAZ_DISCOVERY_TXT_VERSION: u16 = 1;

const TXT_VERSION_KEY: &str = "v";
const TXT_DEVICE_ID_KEY: &str = "device_id";
const TXT_DISPLAY_NAME_KEY: &str = "display_name";
const TXT_IDENTITY_PUBLIC_KEY_KEY: &str = "identity_public_key";
const TXT_FINGERPRINT_KEY: &str = "fingerprint";
const TXT_CAPABILITIES_KEY: &str = "caps";
const TXT_BUILD_VERSION_KEY: &str = "build";

const CAPABILITY_POINTER: &str = "pointer";
const CAPABILITY_KEYBOARD: &str = "keyboard";
const CAPABILITY_CLIPBOARD: &str = "clipboard";
const CAPABILITY_SCREEN_LAYOUT: &str = "screen-layout";
const MDNS_SHUTDOWN_WAIT: Duration = Duration::from_secs(1);

/// Parsed TXT record advertised by one Akraz peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryTxtRecord {
    pub version: u16,
    pub device_id: String,
    pub display_name: Option<String>,
    pub identity_public_key: Option<String>,
    pub fingerprint: Option<String>,
    pub capabilities: CapabilityFlags,
    pub build_version: String,
}

impl DiscoveryTxtRecord {
    /// Build the public TXT payload advertised by a local peer identity.
    pub fn from_device_identity(
        identity: &DeviceIdentity,
        capabilities: CapabilityFlags,
        build_version: impl Into<String>,
    ) -> Self {
        let document = PairingIdentityDocument::from_device_identity(identity, capabilities);

        Self {
            version: AKRAZ_DISCOVERY_TXT_VERSION,
            device_id: document.device_id().to_string(),
            display_name: Some(document.display_name().to_string()),
            identity_public_key: Some(document.identity_public_key().to_string()),
            fingerprint: Some(document.fingerprint().to_string()),
            capabilities: document.capabilities(),
            build_version: build_version.into(),
        }
    }
}

/// One peer candidate after DNS-SD endpoint data and TXT records have been decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub instance_name: String,
    pub host_name: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub txt: DiscoveryTxtRecord,
}

impl DiscoveredPeer {
    /// Return the endpoint used by `session.connect`, preserving resolver address priority.
    pub fn primary_socket_addr(&self) -> Option<SocketAddr> {
        self.addresses
            .first()
            .copied()
            .map(|address| SocketAddr::new(address, self.port))
    }
}

/// Configuration for the Akraz mDNS/DNS-SD discovery backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsDiscoveryConfig {
    pub instance_name: String,
    pub host_name: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub txt: DiscoveryTxtRecord,
}

impl MdnsDiscoveryConfig {
    /// Build a publishable mDNS config from a local discovery TXT record.
    pub fn from_txt_record(
        txt: DiscoveryTxtRecord,
        port: u16,
        addresses: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        Self {
            instance_name: mdns_instance_name(&txt),
            host_name: mdns_host_name(&txt),
            addresses: addresses.into_iter().collect(),
            port,
            txt,
        }
    }
}

/// Running mDNS/DNS-SD publisher and browser for Akraz peers.
pub struct MdnsDiscoveryRuntime {
    daemon: ServiceDaemon,
    events: Receiver<ServiceEvent>,
    service_fullname: String,
    peers: BTreeMap<String, DiscoveredPeer>,
}

impl MdnsDiscoveryRuntime {
    /// Register the local peer and start browsing for Akraz peers.
    pub fn start(config: MdnsDiscoveryConfig) -> Result<Self, MdnsDiscoveryError> {
        let service_info = build_mdns_service_info(&config)?;
        let service_fullname = service_info.get_fullname().to_string();
        let daemon = ServiceDaemon::new().map_err(|error| backend_error("start", error))?;
        let events = match daemon.browse(AKRAZ_DISCOVERY_SERVICE_TYPE) {
            Ok(events) => events,
            Err(error) => {
                wait_for_receiver_result(daemon.shutdown());
                return Err(backend_error("browse", error));
            }
        };

        if let Err(error) = daemon.register(service_info) {
            let _ = daemon.stop_browse(AKRAZ_DISCOVERY_SERVICE_TYPE);
            wait_for_receiver_result(daemon.shutdown());
            return Err(backend_error("register", error));
        }

        Ok(Self {
            daemon,
            events,
            service_fullname,
            peers: BTreeMap::new(),
        })
    }

    /// Poll the discovery backend and return the latest peer snapshot.
    pub fn poll_snapshot(
        &mut self,
        max_wait: Duration,
    ) -> Result<Vec<DiscoveredPeer>, MdnsDiscoveryError> {
        if max_wait.is_zero() {
            self.drain_available_events();
            return Ok(self.peer_snapshot());
        }

        if let Ok(event) = self.events.recv_timeout(max_wait) {
            self.apply_event(event);
            self.drain_available_events();
        }

        Ok(self.peer_snapshot())
    }

    /// Stop browsing, unregister the local peer, and ask the mDNS daemon thread to exit.
    pub fn shutdown(self) -> Result<(), MdnsDiscoveryError> {
        let mut first_error = None;

        if let Err(error) = self.daemon.stop_browse(AKRAZ_DISCOVERY_SERVICE_TYPE) {
            first_error.get_or_insert_with(|| backend_error("stop browse", error));
        }
        match self.daemon.unregister(&self.service_fullname) {
            Ok(receiver) => wait_for_receiver(receiver),
            Err(error) => {
                first_error.get_or_insert_with(|| backend_error("unregister", error));
            }
        }
        match self.daemon.shutdown() {
            Ok(receiver) => wait_for_receiver(receiver),
            Err(error) => {
                first_error.get_or_insert_with(|| backend_error("shutdown", error));
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn drain_available_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.apply_event(event);
        }
    }

    fn apply_event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::ServiceResolved(service) => {
                if let Some(peer) = discovered_peer_from_resolved_service(&service) {
                    self.peers.insert(service.get_fullname().to_string(), peer);
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                self.peers.remove(&fullname);
                self.peers.remove(&fullname.to_ascii_lowercase());
            }
            _ => {}
        }
    }

    fn peer_snapshot(&self) -> Vec<DiscoveredPeer> {
        self.peers.values().cloned().collect()
    }
}

/// Failure returned by the mDNS/DNS-SD discovery backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdnsDiscoveryError {
    InvalidConfig {
        reason: &'static str,
    },
    Backend {
        action: &'static str,
        message: String,
    },
}

impl Display for MdnsDiscoveryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig { reason } => write!(formatter, "invalid mDNS config: {reason}"),
            Self::Backend { action, message } => {
                write!(formatter, "mDNS discovery {action} failed: {message}")
            }
        }
    }
}

impl Error for MdnsDiscoveryError {}

/// Thread-safe snapshot of the latest peers resolved by a discovery backend.
#[derive(Debug, Clone, Default)]
pub struct SharedDiscoveredPeers {
    peers: Arc<Mutex<Vec<DiscoveredPeer>>>,
}

impl SharedDiscoveredPeers {
    /// Create an empty discovered-peer snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a discovered-peer snapshot from an initial peer list.
    pub fn from_peers(peers: Vec<DiscoveredPeer>) -> Self {
        Self {
            peers: Arc::new(Mutex::new(peers)),
        }
    }

    /// Replace the current discovered-peer snapshot.
    pub fn replace(&self, peers: Vec<DiscoveredPeer>) -> Result<(), DiscoveredPeerSnapshotError> {
        *self
            .peers
            .lock()
            .map_err(|_| DiscoveredPeerSnapshotError::Unavailable)? = peers;

        Ok(())
    }

    /// Return an owned snapshot of the current discovered peers.
    pub fn snapshot(&self) -> Result<Vec<DiscoveredPeer>, DiscoveredPeerSnapshotError> {
        self.peers
            .lock()
            .map_err(|_| DiscoveredPeerSnapshotError::Unavailable)
            .map(|peers| peers.clone())
    }
}

/// Errors returned when the shared discovered-peer snapshot cannot be read or updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredPeerSnapshotError {
    Unavailable,
}

impl Display for DiscoveredPeerSnapshotError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("discovered peer snapshot is unavailable"),
        }
    }
}

impl Error for DiscoveredPeerSnapshotError {}

/// Filtering policy applied before a discovered peer is shown as pairable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryPeerFilter {
    pub local_device_id: Option<String>,
    pub required_capabilities: CapabilityFlags,
    pub blocked_device_ids: BTreeSet<String>,
}

impl Default for DiscoveryPeerFilter {
    fn default() -> Self {
        Self {
            local_device_id: None,
            required_capabilities: CapabilityFlags::empty(),
            blocked_device_ids: BTreeSet::new(),
        }
    }
}

/// Peer candidate that is ready to flow into the daemon session-connect surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySessionCandidate {
    pub peer_id: String,
    pub display_name: String,
    pub fingerprint: Option<String>,
    pub peer_document_json: Option<String>,
    pub trusted: bool,
    pub address: SocketAddr,
    pub build_version: String,
    pub capabilities: CapabilityFlags,
}

impl DiscoveryPeerFilter {
    /// Return whether `peer` should be kept as a usable discovery candidate.
    pub fn accept(&self, peer: &DiscoveredPeer) -> Result<(), DiscoveryPeerRejectReason> {
        if self
            .local_device_id
            .as_deref()
            .is_some_and(|local| local == peer.txt.device_id)
        {
            return Err(DiscoveryPeerRejectReason::LocalDevice {
                device_id: peer.txt.device_id.clone(),
            });
        }

        if self.blocked_device_ids.contains(&peer.txt.device_id) {
            return Err(DiscoveryPeerRejectReason::BlockedDevice {
                device_id: peer.txt.device_id.clone(),
            });
        }

        if !self.required_capabilities.is_empty()
            && !peer.txt.capabilities.contains(self.required_capabilities)
        {
            return Err(DiscoveryPeerRejectReason::MissingRequiredCapability {
                required: self.required_capabilities,
                actual: peer.txt.capabilities,
            });
        }

        if peer.host_name.trim().is_empty() || peer.port == 0 || peer.addresses.is_empty() {
            return Err(DiscoveryPeerRejectReason::InvalidEndpoint);
        }

        Ok(())
    }
}

/// Return the accepted peers from a discovery batch.
pub fn filter_discovered_peers<'a>(
    peers: impl IntoIterator<Item = &'a DiscoveredPeer>,
    filter: &DiscoveryPeerFilter,
) -> Vec<&'a DiscoveredPeer> {
    peers
        .into_iter()
        .filter(|peer| filter.accept(peer).is_ok())
        .collect()
}

/// Build daemon session candidates from discovery results and the local trust store.
pub fn build_discovery_session_candidates<'a>(
    peers: impl IntoIterator<Item = &'a DiscoveredPeer>,
    filter: &DiscoveryPeerFilter,
    trusted_peers: impl IntoIterator<Item = &'a TrustedPeerIdentity>,
) -> Vec<DiscoverySessionCandidate> {
    let trusted_peers = trusted_peers
        .into_iter()
        .map(|peer| (peer.peer_id(), peer))
        .collect::<BTreeMap<_, _>>();

    peers
        .into_iter()
        .filter(|peer| filter.accept(peer).is_ok())
        .filter_map(|peer| discovery_session_candidate(peer, &trusted_peers))
        .collect()
}

fn discovery_session_candidate(
    peer: &DiscoveredPeer,
    trusted_peers: &BTreeMap<&str, &TrustedPeerIdentity>,
) -> Option<DiscoverySessionCandidate> {
    let peer_id = peer.txt.device_id.clone();
    let trusted_peer = trusted_peers.get(peer_id.as_str()).copied();
    let address = peer.primary_socket_addr()?;
    let display_name = trusted_peer
        .map(|trusted| trusted.display_name().to_string())
        .or_else(|| peer.txt.display_name.clone())
        .unwrap_or_else(|| discovery_instance_label(&peer.instance_name));
    let pairing_document = trusted_peer
        .is_none()
        .then(|| discovery_pairing_document(peer, &display_name))
        .flatten();
    let peer_document_json = pairing_document
        .as_ref()
        .and_then(|document| serde_json::to_string(document).ok());

    Some(DiscoverySessionCandidate {
        peer_id,
        display_name,
        fingerprint: trusted_peer
            .map(|trusted| trusted.fingerprint().to_string())
            .or_else(|| {
                pairing_document
                    .as_ref()
                    .map(|document| document.fingerprint().to_string())
            }),
        peer_document_json,
        trusted: trusted_peer.is_some(),
        address,
        build_version: peer.txt.build_version.clone(),
        capabilities: peer.txt.capabilities,
    })
}

fn discovery_pairing_document(
    peer: &DiscoveredPeer,
    display_name: &str,
) -> Option<PairingIdentityDocument> {
    PairingIdentityDocument::from_public_wire_fields(
        peer.txt.device_id.clone(),
        display_name.trim(),
        peer.txt.identity_public_key.as_deref()?,
        peer.txt.fingerprint.as_deref()?,
        peer.txt.capabilities,
    )
    .ok()
}

fn discovery_instance_label(instance_name: &str) -> String {
    let label = instance_name
        .trim()
        .trim_end_matches('.')
        .strip_suffix(AKRAZ_DISCOVERY_SERVICE_TYPE.trim_end_matches('.'))
        .unwrap_or_else(|| instance_name.trim().trim_end_matches('.'))
        .trim_end_matches('.');

    if label.is_empty() {
        "Akraz Peer".to_string()
    } else {
        label.to_string()
    }
}

/// Parse DNS-SD TXT entries such as `v=1` and `caps=pointer,keyboard`.
pub fn parse_discovery_txt_record<I, S>(entries: I) -> Result<DiscoveryTxtRecord, DiscoveryTxtError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let fields = collect_txt_fields(entries)?;
    let version = parse_discovery_version(required_txt_field(&fields, TXT_VERSION_KEY)?)?;

    if version != AKRAZ_DISCOVERY_TXT_VERSION {
        return Err(DiscoveryTxtError::UnsupportedVersion { version });
    }

    Ok(DiscoveryTxtRecord {
        version,
        device_id: normalize_device_id(required_txt_field(&fields, TXT_DEVICE_ID_KEY)?)?,
        display_name: optional_txt_field(&fields, TXT_DISPLAY_NAME_KEY)
            .map(normalize_display_name)
            .transpose()?
            .filter(|value| !value.is_empty()),
        identity_public_key: optional_txt_field(&fields, TXT_IDENTITY_PUBLIC_KEY_KEY)
            .map(|value| {
                normalize_no_space_token(value, |value, reason| {
                    DiscoveryTxtError::InvalidIdentityPublicKey { value, reason }
                })
            })
            .transpose()?,
        fingerprint: optional_txt_field(&fields, TXT_FINGERPRINT_KEY)
            .map(|value| {
                normalize_no_space_token(value, |value, reason| {
                    DiscoveryTxtError::InvalidFingerprint { value, reason }
                })
            })
            .transpose()?,
        capabilities: parse_capabilities(required_txt_field(&fields, TXT_CAPABILITIES_KEY)?)?,
        build_version: normalize_build_version(required_txt_field(
            &fields,
            TXT_BUILD_VERSION_KEY,
        )?)?,
    })
}

/// Build the DNS-SD TXT entries that advertise one local Akraz peer.
pub fn build_discovery_txt_record(record: &DiscoveryTxtRecord) -> Vec<String> {
    let mut entries = vec![
        format!("{TXT_VERSION_KEY}={}", record.version),
        format!("{TXT_DEVICE_ID_KEY}={}", record.device_id),
        format!(
            "{TXT_CAPABILITIES_KEY}={}",
            capability_names(record.capabilities).join(",")
        ),
        format!("{TXT_BUILD_VERSION_KEY}={}", record.build_version),
    ];
    if let Some(display_name) = &record.display_name {
        entries.push(format!("{TXT_DISPLAY_NAME_KEY}={display_name}"));
    }
    if let Some(identity_public_key) = &record.identity_public_key {
        entries.push(format!(
            "{TXT_IDENTITY_PUBLIC_KEY_KEY}={identity_public_key}"
        ));
    }
    if let Some(fingerprint) = &record.fingerprint {
        entries.push(format!("{TXT_FINGERPRINT_KEY}={fingerprint}"));
    }

    entries
}

fn build_mdns_service_info(
    config: &MdnsDiscoveryConfig,
) -> Result<ServiceInfo, MdnsDiscoveryError> {
    if config.instance_name.trim().is_empty() {
        return Err(MdnsDiscoveryError::InvalidConfig {
            reason: "instance name is required",
        });
    }
    if !config.host_name.ends_with(".local.") {
        return Err(MdnsDiscoveryError::InvalidConfig {
            reason: "host name must end with .local.",
        });
    }
    if config.port == 0 {
        return Err(MdnsDiscoveryError::InvalidConfig {
            reason: "port must be non-zero",
        });
    }

    let properties = mdns_txt_properties(&config.txt);
    let addresses = config
        .addresses
        .iter()
        .copied()
        .filter(|address| !address.is_unspecified())
        .map(|address| address.to_string())
        .collect::<Vec<_>>();
    let service_info = ServiceInfo::new(
        AKRAZ_DISCOVERY_SERVICE_TYPE,
        &config.instance_name,
        &config.host_name,
        &addresses[..],
        config.port,
        &properties[..],
    )
    .map_err(|error| backend_error("build service info", error))?;

    if addresses.is_empty() {
        Ok(service_info.enable_addr_auto())
    } else {
        Ok(service_info)
    }
}

fn mdns_txt_properties(record: &DiscoveryTxtRecord) -> Vec<(String, String)> {
    build_discovery_txt_record(record)
        .into_iter()
        .filter_map(|entry| {
            entry
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect()
}

fn discovered_peer_from_resolved_service(service: &ResolvedService) -> Option<DiscoveredPeer> {
    let txt = parse_discovery_txt_record(
        service
            .get_properties()
            .iter()
            .map(|property| format!("{}={}", property.key(), property.val_str())),
    )
    .ok()?;
    let mut addresses = service
        .get_addresses()
        .iter()
        .map(|address| address.to_ip_addr())
        .collect::<Vec<_>>();
    addresses.sort();
    addresses.dedup();

    if service.get_hostname().trim().is_empty() || service.get_port() == 0 || addresses.is_empty() {
        return None;
    }

    Some(DiscoveredPeer {
        instance_name: service.get_fullname().to_string(),
        host_name: service.get_hostname().to_string(),
        addresses,
        port: service.get_port(),
        txt,
    })
}

fn mdns_instance_name(record: &DiscoveryTxtRecord) -> String {
    sanitize_mdns_instance_name(&record.device_id)
}

fn sanitize_mdns_instance_name(value: &str) -> String {
    let normalized = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let label = truncate_utf8_bytes(normalized.trim_matches('.'), 63);

    if label.is_empty() {
        "Akraz Peer".to_string()
    } else {
        label
    }
}

fn mdns_host_name(record: &DiscoveryTxtRecord) -> String {
    format!("{}.local.", sanitize_dns_label(&record.device_id))
}

fn sanitize_dns_label(value: &str) -> String {
    let mut label = String::new();
    let mut previous_was_dash = false;

    for character in value.chars().flat_map(char::to_lowercase) {
        let character = if character.is_ascii_alphanumeric() {
            character
        } else {
            '-'
        };
        if character == '-' && previous_was_dash {
            continue;
        }
        label.push(character);
        previous_was_dash = character == '-';
    }

    let label = truncate_utf8_bytes(label.trim_matches('-'), 63);
    let label = label.trim_matches('-');
    if label.is_empty() {
        "akraz-peer".to_string()
    } else {
        label.to_string()
    }
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    let mut truncated = String::new();
    for character in value.chars() {
        if truncated.len() + character.len_utf8() > max_bytes {
            break;
        }
        truncated.push(character);
    }
    truncated
}

fn backend_error(action: &'static str, error: mdns_sd::Error) -> MdnsDiscoveryError {
    MdnsDiscoveryError::Backend {
        action,
        message: error.to_string(),
    }
}

fn wait_for_receiver_result<T>(result: mdns_sd::Result<Receiver<T>>) {
    if let Ok(receiver) = result {
        wait_for_receiver(receiver);
    }
}

fn wait_for_receiver<T>(receiver: Receiver<T>) {
    let _ = receiver.recv_timeout(MDNS_SHUTDOWN_WAIT);
}

fn collect_txt_fields<I, S>(entries: I) -> Result<BTreeMap<String, String>, DiscoveryTxtError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut fields = BTreeMap::new();

    for entry in entries {
        let entry = entry.as_ref();
        let Some((key, value)) = entry.split_once('=') else {
            return Err(DiscoveryTxtError::MalformedEntry {
                entry: entry.to_string(),
            });
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(DiscoveryTxtError::EmptyKey {
                entry: entry.to_string(),
            });
        }

        let normalized_key = key.to_ascii_lowercase();
        if fields
            .insert(normalized_key.clone(), value.trim().to_string())
            .is_some()
        {
            return Err(DiscoveryTxtError::DuplicateField {
                key: normalized_key,
            });
        }
    }

    Ok(fields)
}

fn required_txt_field<'a>(
    fields: &'a BTreeMap<String, String>,
    key: &'static str,
) -> Result<&'a str, DiscoveryTxtError> {
    fields
        .get(key)
        .map(String::as_str)
        .ok_or(DiscoveryTxtError::MissingField { key })
}

fn optional_txt_field<'a>(
    fields: &'a BTreeMap<String, String>,
    key: &'static str,
) -> Option<&'a str> {
    fields.get(key).map(String::as_str)
}

fn parse_discovery_version(value: &str) -> Result<u16, DiscoveryTxtError> {
    value
        .parse::<u16>()
        .map_err(|_| DiscoveryTxtError::InvalidVersion {
            value: value.to_string(),
        })
}

fn normalize_device_id(value: &str) -> Result<String, DiscoveryTxtError> {
    normalize_no_space_token(value, |value, reason| DiscoveryTxtError::InvalidDeviceId {
        value,
        reason,
    })
    .and_then(|device_id| {
        if device_id.contains(':') || device_id.contains('@') {
            return Err(DiscoveryTxtError::InvalidDeviceId {
                value: device_id,
                reason: "device_id cannot contain ':' or '@'",
            });
        }

        Ok(device_id)
    })
}

fn normalize_build_version(value: &str) -> Result<String, DiscoveryTxtError> {
    normalize_no_space_token(value, |value, reason| {
        DiscoveryTxtError::InvalidBuildVersion { value, reason }
    })
}

fn normalize_display_name(value: &str) -> Result<String, DiscoveryTxtError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.chars().any(char::is_control) {
        return Err(DiscoveryTxtError::InvalidDisplayName {
            value: value.to_string(),
            reason: "field cannot contain control characters",
        });
    }

    Ok(value.to_string())
}

fn normalize_no_space_token(
    value: &str,
    error: fn(String, &'static str) -> DiscoveryTxtError,
) -> Result<String, DiscoveryTxtError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(error(value.to_string(), "field is required"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(error(value.to_string(), "field cannot contain whitespace"));
    }
    if value.chars().any(char::is_control) {
        return Err(error(
            value.to_string(),
            "field cannot contain control characters",
        ));
    }

    Ok(value.to_string())
}

fn parse_capabilities(value: &str) -> Result<CapabilityFlags, DiscoveryTxtError> {
    let mut capabilities = CapabilityFlags::empty();
    let mut saw_capability = false;

    for raw_capability in value.split(',') {
        let capability = raw_capability.trim();
        if capability.is_empty() {
            return Err(DiscoveryTxtError::InvalidCapability {
                value: raw_capability.to_string(),
            });
        }

        saw_capability = true;
        capabilities |= match capability {
            CAPABILITY_POINTER => CapabilityFlags::POINTER,
            CAPABILITY_KEYBOARD => CapabilityFlags::KEYBOARD,
            CAPABILITY_CLIPBOARD => CapabilityFlags::CLIPBOARD,
            CAPABILITY_SCREEN_LAYOUT | "screen_layout" => CapabilityFlags::SCREEN_LAYOUT,
            _ => {
                return Err(DiscoveryTxtError::InvalidCapability {
                    value: capability.to_string(),
                });
            }
        };
    }

    if !saw_capability {
        return Err(DiscoveryTxtError::EmptyCapabilityList);
    }

    Ok(capabilities)
}

fn capability_names(capabilities: CapabilityFlags) -> Vec<&'static str> {
    let mut names = Vec::new();
    if capabilities.contains(CapabilityFlags::POINTER) {
        names.push(CAPABILITY_POINTER);
    }
    if capabilities.contains(CapabilityFlags::KEYBOARD) {
        names.push(CAPABILITY_KEYBOARD);
    }
    if capabilities.contains(CapabilityFlags::CLIPBOARD) {
        names.push(CAPABILITY_CLIPBOARD);
    }
    if capabilities.contains(CapabilityFlags::SCREEN_LAYOUT) {
        names.push(CAPABILITY_SCREEN_LAYOUT);
    }

    names
}

/// Failure returned when discovery TXT records are malformed or unsupported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryTxtError {
    MissingField { key: &'static str },
    DuplicateField { key: String },
    MalformedEntry { entry: String },
    EmptyKey { entry: String },
    InvalidVersion { value: String },
    UnsupportedVersion { version: u16 },
    InvalidDeviceId { value: String, reason: &'static str },
    InvalidDisplayName { value: String, reason: &'static str },
    InvalidIdentityPublicKey { value: String, reason: &'static str },
    InvalidFingerprint { value: String, reason: &'static str },
    InvalidBuildVersion { value: String, reason: &'static str },
    InvalidCapability { value: String },
    EmptyCapabilityList,
}

impl Display for DiscoveryTxtError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField { key } => write!(formatter, "missing discovery TXT field {key}"),
            Self::DuplicateField { key } => {
                write!(formatter, "duplicate discovery TXT field {key}")
            }
            Self::MalformedEntry { entry } => {
                write!(formatter, "malformed discovery TXT entry {entry:?}")
            }
            Self::EmptyKey { entry } => write!(formatter, "empty discovery TXT key in {entry:?}"),
            Self::InvalidVersion { value } => {
                write!(formatter, "invalid discovery TXT version {value:?}")
            }
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported discovery TXT version {version}")
            }
            Self::InvalidDeviceId { value, reason } => {
                write!(formatter, "invalid discovery device_id {value:?}: {reason}")
            }
            Self::InvalidDisplayName { value, reason } => {
                write!(
                    formatter,
                    "invalid discovery display_name {value:?}: {reason}"
                )
            }
            Self::InvalidIdentityPublicKey { value, reason } => {
                write!(
                    formatter,
                    "invalid discovery identity_public_key {value:?}: {reason}"
                )
            }
            Self::InvalidFingerprint { value, reason } => {
                write!(
                    formatter,
                    "invalid discovery fingerprint {value:?}: {reason}"
                )
            }
            Self::InvalidBuildVersion { value, reason } => {
                write!(formatter, "invalid discovery build {value:?}: {reason}")
            }
            Self::InvalidCapability { value } => {
                write!(formatter, "invalid discovery capability {value:?}")
            }
            Self::EmptyCapabilityList => formatter.write_str("empty discovery capability list"),
        }
    }
}

impl Error for DiscoveryTxtError {}

/// Reason a syntactically valid discovered peer is hidden from pairable results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryPeerRejectReason {
    LocalDevice {
        device_id: String,
    },
    BlockedDevice {
        device_id: String,
    },
    MissingRequiredCapability {
        required: CapabilityFlags,
        actual: CapabilityFlags,
    },
    InvalidEndpoint,
}

impl Display for DiscoveryPeerRejectReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalDevice { device_id } => {
                write!(formatter, "discovered peer {device_id} is the local device")
            }
            Self::BlockedDevice { device_id } => {
                write!(formatter, "discovered peer {device_id} is blocked")
            }
            Self::MissingRequiredCapability { required, actual } => write!(
                formatter,
                "discovered peer is missing required capabilities: required {}, actual {}",
                required.bits(),
                actual.bits()
            ),
            Self::InvalidEndpoint => formatter.write_str("discovered peer endpoint is invalid"),
        }
    }
}

impl Error for DiscoveryPeerRejectReason {}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, net::IpAddr};

    use akraz_identity::{
        DeviceIdentity, Ed25519IdentityKey, PairingIdentityDocument, fingerprint_for_public_key,
    };
    use akraz_protocol::CapabilityFlags;

    use super::{
        AKRAZ_DISCOVERY_SERVICE_TYPE, AKRAZ_DISCOVERY_TXT_VERSION, DiscoveredPeer,
        DiscoveryPeerFilter, DiscoveryPeerRejectReason, DiscoverySessionCandidate,
        DiscoveryTxtError, DiscoveryTxtRecord, MdnsDiscoveryConfig, MdnsDiscoveryError,
        SharedDiscoveredPeers, build_discovery_session_candidates, build_discovery_txt_record,
        build_mdns_service_info, discovered_peer_from_resolved_service, filter_discovered_peers,
        parse_discovery_txt_record,
    };

    fn txt_record() -> DiscoveryTxtRecord {
        DiscoveryTxtRecord {
            version: AKRAZ_DISCOVERY_TXT_VERSION,
            device_id: "linux-laptop".to_string(),
            display_name: None,
            identity_public_key: None,
            fingerprint: None,
            capabilities: CapabilityFlags::POINTER
                | CapabilityFlags::KEYBOARD
                | CapabilityFlags::SCREEN_LAYOUT,
            build_version: "0.5.7".to_string(),
        }
    }

    fn peer(device_id: &str, capabilities: CapabilityFlags) -> DiscoveredPeer {
        DiscoveredPeer {
            instance_name: format!("{device_id}.{AKRAZ_DISCOVERY_SERVICE_TYPE}"),
            host_name: format!("{device_id}.local."),
            addresses: vec!["127.0.0.1".parse().expect("loopback address")],
            port: 4455,
            txt: DiscoveryTxtRecord {
                device_id: device_id.to_string(),
                capabilities,
                ..txt_record()
            },
        }
    }

    #[test]
    fn shared_discovered_peers_returns_owned_snapshots() {
        let source = SharedDiscoveredPeers::from_peers(vec![peer(
            "linux-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        )]);

        let mut snapshot = source.snapshot().expect("discovery snapshot");
        snapshot[0].txt.device_id = "mutated-copy".to_string();

        assert_eq!(
            source
                .snapshot()
                .expect("second discovery snapshot")
                .into_iter()
                .map(|peer| peer.txt.device_id)
                .collect::<Vec<_>>(),
            vec!["linux-laptop"]
        );
    }

    #[test]
    fn shared_discovered_peers_replaces_current_snapshot() {
        let source = SharedDiscoveredPeers::new();

        source
            .replace(vec![peer(
                "linux-laptop",
                CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            )])
            .expect("replace discovery snapshot");
        source
            .replace(vec![peer("keyboard-only", CapabilityFlags::KEYBOARD)])
            .expect("replace discovery snapshot again");

        assert_eq!(
            source
                .snapshot()
                .expect("current discovery snapshot")
                .into_iter()
                .map(|peer| peer.txt.device_id)
                .collect::<Vec<_>>(),
            vec!["keyboard-only"]
        );
    }

    #[test]
    fn parses_dns_sd_txt_record_contract() {
        let record = parse_discovery_txt_record([
            "v=1",
            "device_id=linux-laptop",
            "caps=pointer,keyboard,screen-layout",
            "build=0.5.7",
        ])
        .expect("parse TXT record");

        assert_eq!(record, txt_record());
    }

    #[test]
    fn parses_optional_public_pairing_txt_fields() {
        let document = public_pairing_document("linux-laptop", "Linux Laptop");
        let record = parse_discovery_txt_record([
            "v=1",
            "device_id=linux-laptop",
            "display_name=Linux Laptop",
            &format!("identity_public_key={}", document.identity_public_key()),
            &format!("fingerprint={}", document.fingerprint()),
            "caps=pointer,keyboard",
            "build=0.5.7",
        ])
        .expect("parse TXT record");

        assert_eq!(record.display_name.as_deref(), Some("Linux Laptop"));
        assert_eq!(
            record.identity_public_key.as_deref(),
            Some(document.identity_public_key())
        );
        assert_eq!(record.fingerprint.as_deref(), Some(document.fingerprint()));
    }

    #[test]
    fn builds_deterministic_txt_records_for_publishers() {
        assert_eq!(
            build_discovery_txt_record(&txt_record()),
            vec![
                "v=1",
                "device_id=linux-laptop",
                "caps=pointer,keyboard,screen-layout",
                "build=0.5.7",
            ]
        );
    }

    #[test]
    fn builds_optional_public_pairing_txt_records_for_publishers() {
        let document = public_pairing_document("linux-laptop", "Linux Laptop");
        let record = DiscoveryTxtRecord {
            display_name: Some("Linux Laptop".to_string()),
            identity_public_key: Some(document.identity_public_key().to_string()),
            fingerprint: Some(document.fingerprint().to_string()),
            capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            ..txt_record()
        };

        assert_eq!(
            build_discovery_txt_record(&record),
            vec![
                "v=1".to_string(),
                "device_id=linux-laptop".to_string(),
                "caps=pointer,keyboard".to_string(),
                "build=0.5.7".to_string(),
                "display_name=Linux Laptop".to_string(),
                format!("identity_public_key={}", document.identity_public_key()),
                format!("fingerprint={}", document.fingerprint()),
            ]
        );
    }

    #[test]
    fn builds_advertised_txt_record_from_local_identity() {
        let secret_key = Ed25519IdentityKey::generate();
        let public_key = secret_key.public_key_bytes();
        let identity = DeviceIdentity::new(
            "linux-laptop",
            "Linux Laptop",
            public_key,
            fingerprint_for_public_key(&public_key),
        );
        let document = PairingIdentityDocument::from_device_identity(
            &identity,
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );

        let record = DiscoveryTxtRecord::from_device_identity(
            &identity,
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            "0.8.0",
        );

        assert_eq!(record.version, AKRAZ_DISCOVERY_TXT_VERSION);
        assert_eq!(record.device_id, "linux-laptop");
        assert_eq!(record.display_name.as_deref(), Some("Linux Laptop"));
        assert_eq!(
            record.identity_public_key.as_deref(),
            Some(document.identity_public_key())
        );
        assert_eq!(record.fingerprint.as_deref(), Some(document.fingerprint()));
        assert_eq!(
            record.capabilities,
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
        );
        assert_eq!(record.build_version, "0.8.0");
        assert_eq!(
            build_discovery_txt_record(&record),
            vec![
                "v=1".to_string(),
                "device_id=linux-laptop".to_string(),
                "caps=pointer,keyboard".to_string(),
                "build=0.8.0".to_string(),
                "display_name=Linux Laptop".to_string(),
                format!("identity_public_key={}", document.identity_public_key()),
                format!("fingerprint={}", document.fingerprint()),
            ]
        );
    }

    #[test]
    fn builds_mdns_service_info_from_discovery_txt_record() {
        let config = MdnsDiscoveryConfig::from_txt_record(
            DiscoveryTxtRecord {
                display_name: Some("Linux Laptop".to_string()),
                capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
                ..txt_record()
            },
            4455,
            ["192.168.1.23".parse().expect("LAN address")],
        );

        let service_info = build_mdns_service_info(&config).expect("mDNS service info");

        assert_eq!(config.instance_name, "linux-laptop");
        assert_eq!(config.host_name, "linux-laptop.local.");
        assert_eq!(
            service_info.get_fullname(),
            "linux-laptop._akraz._tcp.local."
        );
        assert_eq!(service_info.get_hostname(), "linux-laptop.local.");
        assert_eq!(service_info.get_port(), 4455);
        assert_eq!(
            service_info
                .get_addresses()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec!["192.168.1.23".parse::<IpAddr>().expect("LAN address")]
        );

        let resolved = service_info.as_resolved_service();
        let discovered = discovered_peer_from_resolved_service(&resolved).expect("discovered peer");

        assert_eq!(discovered.instance_name, "linux-laptop._akraz._tcp.local.");
        assert_eq!(discovered.host_name, "linux-laptop.local.");
        assert_eq!(discovered.port, 4455);
        assert_eq!(
            discovered.addresses,
            vec!["192.168.1.23".parse::<IpAddr>().expect("LAN address")]
        );
        assert_eq!(
            discovered.txt.capabilities,
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
        );
        assert_eq!(discovered.txt.display_name.as_deref(), Some("Linux Laptop"));
    }

    #[test]
    fn mdns_service_info_uses_auto_addresses_for_unspecified_listener() {
        let config = MdnsDiscoveryConfig::from_txt_record(
            txt_record(),
            4455,
            ["0.0.0.0".parse().expect("unspecified address")],
        );

        let service_info = build_mdns_service_info(&config).expect("mDNS service info");

        assert!(service_info.get_addresses().is_empty());
        assert!(service_info.is_addr_auto());
    }

    #[test]
    fn mdns_service_info_rejects_zero_port() {
        let config = MdnsDiscoveryConfig::from_txt_record(txt_record(), 0, Vec::<IpAddr>::new());

        assert_eq!(
            build_mdns_service_info(&config).map(|_| ()),
            Err(MdnsDiscoveryError::InvalidConfig {
                reason: "port must be non-zero",
            })
        );
    }

    #[test]
    fn malformed_mdns_resolved_service_is_ignored() {
        let service_info = mdns_sd::ServiceInfo::new(
            AKRAZ_DISCOVERY_SERVICE_TYPE,
            "Broken Peer",
            "broken-peer.local.",
            "192.168.1.44",
            4455,
            &[("v", "1"), ("device_id", "broken-peer"), ("build", "0.8.0")][..],
        )
        .expect("broken mDNS service info");

        assert!(
            discovered_peer_from_resolved_service(&service_info.as_resolved_service()).is_none()
        );
    }

    #[test]
    fn rejects_missing_duplicate_unsupported_and_unknown_txt_fields() {
        assert_eq!(
            parse_discovery_txt_record(["v=1", "device_id=linux-laptop", "build=0.5.7"]),
            Err(DiscoveryTxtError::MissingField { key: "caps" })
        );
        assert_eq!(
            parse_discovery_txt_record([
                "v=1",
                "V=1",
                "device_id=linux-laptop",
                "caps=pointer",
                "build=0.5.7",
            ]),
            Err(DiscoveryTxtError::DuplicateField {
                key: "v".to_string(),
            })
        );
        assert_eq!(
            parse_discovery_txt_record([
                "v=2",
                "device_id=linux-laptop",
                "caps=pointer",
                "build=0.5.7",
            ]),
            Err(DiscoveryTxtError::UnsupportedVersion { version: 2 })
        );
        assert_eq!(
            parse_discovery_txt_record([
                "v=1",
                "device_id=linux-laptop",
                "caps=pointer,text",
                "build=0.5.7",
            ]),
            Err(DiscoveryTxtError::InvalidCapability {
                value: "text".to_string(),
            })
        );
    }

    #[test]
    fn rejects_device_ids_that_cannot_flow_into_session_contracts() {
        assert_eq!(
            parse_discovery_txt_record([
                "v=1",
                "device_id=linux laptop",
                "caps=pointer",
                "build=0.5.7",
            ]),
            Err(DiscoveryTxtError::InvalidDeviceId {
                value: "linux laptop".to_string(),
                reason: "field cannot contain whitespace",
            })
        );
        assert_eq!(
            parse_discovery_txt_record([
                "v=1",
                "device_id=linux:laptop",
                "caps=pointer",
                "build=0.5.7",
            ]),
            Err(DiscoveryTxtError::InvalidDeviceId {
                value: "linux:laptop".to_string(),
                reason: "device_id cannot contain ':' or '@'",
            })
        );
    }

    #[test]
    fn filters_local_blocked_incapable_and_invalid_endpoint_peers() {
        let filter = DiscoveryPeerFilter {
            local_device_id: Some("windows-desktop".to_string()),
            required_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            blocked_device_ids: BTreeSet::from(["blocked-laptop".to_string()]),
        };

        assert_eq!(
            filter.accept(&peer(
                "windows-desktop",
                CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
            )),
            Err(DiscoveryPeerRejectReason::LocalDevice {
                device_id: "windows-desktop".to_string(),
            })
        );
        assert_eq!(
            filter.accept(&peer(
                "blocked-laptop",
                CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
            )),
            Err(DiscoveryPeerRejectReason::BlockedDevice {
                device_id: "blocked-laptop".to_string(),
            })
        );
        assert_eq!(
            filter.accept(&peer("keyboard-only", CapabilityFlags::KEYBOARD)),
            Err(DiscoveryPeerRejectReason::MissingRequiredCapability {
                required: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
                actual: CapabilityFlags::KEYBOARD,
            })
        );

        let mut invalid_endpoint = peer(
            "linux-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        invalid_endpoint.addresses.clear();
        assert_eq!(
            filter.accept(&invalid_endpoint),
            Err(DiscoveryPeerRejectReason::InvalidEndpoint)
        );
    }

    #[test]
    fn returns_accepted_discovered_peers_in_original_order() {
        let local = peer(
            "windows-desktop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let accepted = peer(
            "linux-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let incapable = peer("keyboard-only", CapabilityFlags::KEYBOARD);
        let filter = DiscoveryPeerFilter {
            local_device_id: Some("windows-desktop".to_string()),
            required_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            blocked_device_ids: BTreeSet::new(),
        };
        let peers = vec![local, accepted, incapable];

        let accepted_peers = filter_discovered_peers(&peers, &filter);

        assert_eq!(accepted_peers, vec![&peers[1]]);
    }

    #[test]
    fn builds_session_candidates_with_trusted_peer_metadata() {
        let trusted = akraz_identity::TrustedPeerIdentity::new(
            "linux-laptop",
            "Linux Laptop",
            b"public-key".to_vec(),
            "AKRZ-TRUSTED",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let discovered = peer(
            "linux-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        let filter = DiscoveryPeerFilter {
            required_capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            ..DiscoveryPeerFilter::default()
        };

        let candidates = build_discovery_session_candidates([&discovered], &filter, [&trusted]);

        assert_eq!(
            candidates,
            vec![DiscoverySessionCandidate {
                peer_id: "linux-laptop".to_string(),
                display_name: "Linux Laptop".to_string(),
                fingerprint: Some("AKRZ-TRUSTED".to_string()),
                peer_document_json: None,
                trusted: true,
                address: "127.0.0.1:4455".parse().expect("candidate address"),
                build_version: "0.5.7".to_string(),
                capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            }]
        );
    }

    #[test]
    fn keeps_unpaired_discovery_candidates_explicitly_untrusted() {
        let discovered = peer(
            "new-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );

        let candidates = build_discovery_session_candidates(
            [&discovered],
            &DiscoveryPeerFilter::default(),
            Vec::<&akraz_identity::TrustedPeerIdentity>::new(),
        );

        assert_eq!(
            candidates,
            vec![DiscoverySessionCandidate {
                peer_id: "new-laptop".to_string(),
                display_name: "new-laptop".to_string(),
                fingerprint: None,
                peer_document_json: None,
                trusted: false,
                address: "127.0.0.1:4455".parse().expect("candidate address"),
                build_version: "0.5.7".to_string(),
                capabilities: CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
            }]
        );
    }

    #[test]
    fn builds_registerable_discovery_candidates_from_public_pairing_txt_fields() {
        let document = public_pairing_document("new-laptop", "New Laptop");
        let mut discovered = peer(
            "new-laptop",
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        );
        discovered.txt.display_name = Some("New Laptop".to_string());
        discovered.txt.identity_public_key = Some(document.identity_public_key().to_string());
        discovered.txt.fingerprint = Some(document.fingerprint().to_string());

        let candidates = build_discovery_session_candidates(
            [&discovered],
            &DiscoveryPeerFilter::default(),
            Vec::<&akraz_identity::TrustedPeerIdentity>::new(),
        );

        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.display_name, "New Laptop");
        assert_eq!(
            candidate.fingerprint.as_deref(),
            Some(document.fingerprint())
        );
        assert!(!candidate.trusted);
        let peer_document_json = candidate
            .peer_document_json
            .as_deref()
            .expect("peer document JSON");
        let decoded: PairingIdentityDocument =
            serde_json::from_str(peer_document_json).expect("candidate document JSON");
        assert_eq!(decoded.device_id(), "new-laptop");
        assert_eq!(decoded.display_name(), "New Laptop");
        assert_eq!(
            decoded.identity_public_key(),
            document.identity_public_key()
        );
        assert_eq!(decoded.fingerprint(), document.fingerprint());
    }

    fn public_pairing_document(device_id: &str, display_name: &str) -> PairingIdentityDocument {
        let secret_key = Ed25519IdentityKey::generate();
        let public_key = secret_key.public_key_bytes();
        let identity = DeviceIdentity::new(
            device_id,
            display_name,
            public_key,
            fingerprint_for_public_key(&public_key),
        );
        PairingIdentityDocument::from_device_identity(
            &identity,
            CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD,
        )
    }
}
