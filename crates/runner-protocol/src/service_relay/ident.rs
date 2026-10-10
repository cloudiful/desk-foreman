//! Canonical relay stream identifiers (issue 749 P2a).
//!
//! Both endpoints identify a relayed stream by a UUID that also appears
//! verbatim as the 16-byte prefix of every data frame. This is a thin
//! adapter over the workspace `uuid` crate: it owns the wire shape, and
//! delegates the value semantics.
//!
//! The textual form is the canonical 36-character hyphenated form, in
//! lowercase. Parsing is two steps and both matter: the character shape is
//! checked first so only the canonical form reaches the parser, then the
//! string is handed to `Uuid::parse_str`. That keeps the wire strict — a
//! braced, unhyphenated, or wrongly punctuated identifier is refused rather
//! than coerced — without re-implementing hex parsing.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::service_relay::RelayProtocolError;

/// Wire width of a relay identifier in bytes.
pub const RELAY_ID_BYTES: usize = 16;
/// Length of the canonical textual form, separators included.
const CANONICAL_LEN: usize = 36;
/// Byte offsets of the separators in the canonical form.
const HYPHEN_OFFSETS: [usize; 4] = [8, 13, 18, 23];

/// A relay stream identifier.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelayId(Uuid);

impl RelayId {
    pub fn from_bytes(bytes: [u8; RELAY_ID_BYTES]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; RELAY_ID_BYTES] {
        self.0.as_bytes()
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Canonical lowercase hyphenated text form.
    pub fn to_canonical(self) -> String {
        self.0.hyphenated().to_string()
    }
}

impl std::fmt::Display for RelayId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_canonical())
    }
}

impl std::fmt::Debug for RelayId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_canonical())
    }
}

impl Serialize for RelayId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_canonical())
    }
}

impl<'de> Deserialize<'de> for RelayId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        parse_relay_id(&raw).map_err(serde::de::Error::custom)
    }
}

/// Parse the canonical textual form.
///
/// Shape is validated first so that only the hyphenated canonical layout
/// reaches the UUID parser; the parser then normalises case, so an
/// upper-case wire form is accepted and stored canonically lowercase.
pub fn parse_relay_id(raw: &str) -> Result<RelayId, RelayProtocolError> {
    if !has_canonical_shape(raw) {
        return Err(RelayProtocolError::InvalidIdentifier);
    }
    Uuid::parse_str(raw)
        .map(RelayId)
        .map_err(|_| RelayProtocolError::InvalidIdentifier)
}

/// Length, separator placement, and hex digits — nothing about the value
/// itself, which the parser owns.
fn has_canonical_shape(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != CANONICAL_LEN {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if HYPHEN_OFFSETS.contains(&index) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}
