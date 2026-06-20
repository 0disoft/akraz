//! Discovery contracts for local-network Akraz peers.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

use akraz_protocol::CapabilityFlags;

/// DNS-SD service type used by Akraz mDNS discovery.
pub const AKRAZ_DISCOVERY_SERVICE_TYPE: &str = "_akraz._tcp.local.";

/// Current Akraz discovery TXT contract version.
pub const AKRAZ_DISCOVERY_TXT_VERSION: u16 = 1;

const TXT_VERSION_KEY: &str = "v";
const TXT_DEVICE_ID_KEY: &str = "device_id";
const TXT_CAPABILITIES_KEY: &str = "caps";
const TXT_BUILD_VERSION_KEY: &str = "build";

const CAPABILITY_POINTER: &str = "pointer";
const CAPABILITY_KEYBOARD: &str = "keyboard";
const CAPABILITY_CLIPBOARD: &str = "clipboard";
const CAPABILITY_SCREEN_LAYOUT: &str = "screen-layout";

/// Parsed TXT record advertised by one Akraz peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryTxtRecord {
    pub version: u16,
    pub device_id: String,
    pub capabilities: CapabilityFlags,
    pub build_version: String,
}

/// One peer candidate after DNS-SD endpoint data and TXT records have been decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub instance_name: String,
    pub host_name: String,
    pub port: u16,
    pub txt: DiscoveryTxtRecord,
}

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

        if peer.host_name.trim().is_empty() || peer.port == 0 {
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
        capabilities: parse_capabilities(required_txt_field(&fields, TXT_CAPABILITIES_KEY)?)?,
        build_version: normalize_build_version(required_txt_field(
            &fields,
            TXT_BUILD_VERSION_KEY,
        )?)?,
    })
}

/// Build the DNS-SD TXT entries that advertise one local Akraz peer.
pub fn build_discovery_txt_record(record: &DiscoveryTxtRecord) -> Vec<String> {
    vec![
        format!("{TXT_VERSION_KEY}={}", record.version),
        format!("{TXT_DEVICE_ID_KEY}={}", record.device_id),
        format!(
            "{TXT_CAPABILITIES_KEY}={}",
            capability_names(record.capabilities).join(",")
        ),
        format!("{TXT_BUILD_VERSION_KEY}={}", record.build_version),
    ]
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
    use std::collections::BTreeSet;

    use akraz_protocol::CapabilityFlags;

    use super::{
        AKRAZ_DISCOVERY_SERVICE_TYPE, AKRAZ_DISCOVERY_TXT_VERSION, DiscoveredPeer,
        DiscoveryPeerFilter, DiscoveryPeerRejectReason, DiscoveryTxtError, DiscoveryTxtRecord,
        build_discovery_txt_record, filter_discovered_peers, parse_discovery_txt_record,
    };

    fn txt_record() -> DiscoveryTxtRecord {
        DiscoveryTxtRecord {
            version: AKRAZ_DISCOVERY_TXT_VERSION,
            device_id: "linux-laptop".to_string(),
            capabilities: CapabilityFlags::POINTER
                | CapabilityFlags::KEYBOARD
                | CapabilityFlags::SCREEN_LAYOUT,
            build_version: "0.5.2".to_string(),
        }
    }

    fn peer(device_id: &str, capabilities: CapabilityFlags) -> DiscoveredPeer {
        DiscoveredPeer {
            instance_name: format!("{device_id}.{AKRAZ_DISCOVERY_SERVICE_TYPE}"),
            host_name: format!("{device_id}.local."),
            port: 4455,
            txt: DiscoveryTxtRecord {
                device_id: device_id.to_string(),
                capabilities,
                ..txt_record()
            },
        }
    }

    #[test]
    fn parses_dns_sd_txt_record_contract() {
        let record = parse_discovery_txt_record([
            "v=1",
            "device_id=linux-laptop",
            "caps=pointer,keyboard,screen-layout",
            "build=0.5.2",
        ])
        .expect("parse TXT record");

        assert_eq!(record, txt_record());
    }

    #[test]
    fn builds_deterministic_txt_records_for_publishers() {
        assert_eq!(
            build_discovery_txt_record(&txt_record()),
            vec![
                "v=1",
                "device_id=linux-laptop",
                "caps=pointer,keyboard,screen-layout",
                "build=0.5.2",
            ]
        );
    }

    #[test]
    fn rejects_missing_duplicate_unsupported_and_unknown_txt_fields() {
        assert_eq!(
            parse_discovery_txt_record(["v=1", "device_id=linux-laptop", "build=0.5.2"]),
            Err(DiscoveryTxtError::MissingField { key: "caps" })
        );
        assert_eq!(
            parse_discovery_txt_record([
                "v=1",
                "V=1",
                "device_id=linux-laptop",
                "caps=pointer",
                "build=0.5.2",
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
                "build=0.5.2",
            ]),
            Err(DiscoveryTxtError::UnsupportedVersion { version: 2 })
        );
        assert_eq!(
            parse_discovery_txt_record([
                "v=1",
                "device_id=linux-laptop",
                "caps=pointer,text",
                "build=0.5.2",
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
                "build=0.5.2",
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
                "build=0.5.2",
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
        invalid_endpoint.port = 0;
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
}
