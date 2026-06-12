//! Wire protocol constants and versioning for akraz peers.

/// First supported protocol major version.
pub const PROTOCOL_MAJOR: u16 = 1;

/// First supported protocol minor version.
pub const PROTOCOL_MINOR: u16 = 0;

/// Protocol version exchanged during session negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[cfg(test)]
mod tests {
    use super::ProtocolVersion;

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
}
