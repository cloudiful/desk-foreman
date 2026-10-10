//! Relay header entries, both directions (issue 749 P2b).
//!
//! Split from [`super::bounds`] so the whole rule set stays readable. This
//! owns the only request headers that may cross the relay and the wire
//! encoding of a permitted entry. Everything else — the caller's
//! authorization, cookies, forwarding and hop headers, and anything else an
//! agent could be tricked into treating as an identity — is dropped here and
//! never reaches the tunnel.

use axum::http::{HeaderMap, header};

use super::BoundsRefusal;

/// Largest value accepted for one forwarded request header.
pub const MAX_SERVICE_HEADER_VALUE_BYTES: usize = 2 * 1024;

/// Encode one permitted request header as a single relay entry.
///
/// The frozen frame carries a list of strings and names the headers it will
/// let cross; it has no field for a value. An entry therefore carries
/// `name: value` so a caller-supplied content type actually reaches the
/// agent, and a bare name carries nothing and is dropped on the far side.
/// This is additive: the frozen frame shape is unchanged, and a peer that
/// only understands names sees names.
pub fn header_entry(name: &str, value: &str) -> String {
    format!("{name}: {value}")
}

/// Split one relay header entry back into its name and value.
///
/// Only the first colon separates them, so a value may contain colons.
pub fn parse_header_entry(raw: &str) -> Option<(String, String)> {
    let (name, value) = raw.split_once(':')?;
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() || value.is_empty() {
        return None;
    }
    Some((name.to_ascii_lowercase(), value.to_string()))
}

/// The only request headers that may cross the relay.
///
/// Everything else — the caller's authorization, cookies, forwarding and
/// hop headers, and anything else an agent could be tricked into treating as
/// an identity — is dropped here and never reaches the tunnel.
pub fn permitted_request_headers(
    headers: &HeaderMap,
) -> Result<Vec<(String, String)>, BoundsRefusal> {
    let mut permitted = Vec::new();
    for name in [header::CONTENT_TYPE, header::ACCEPT] {
        let Some(value) = headers.get(&name) else {
            continue;
        };
        let value = value.to_str().map_err(|_| BoundsRefusal::InvalidHeader)?;
        if value.len() > MAX_SERVICE_HEADER_VALUE_BYTES
            || value.chars().any(|character| character.is_control())
        {
            return Err(BoundsRefusal::InvalidHeader);
        }
        permitted.push((name.as_str().to_ascii_lowercase(), value.to_string()));
    }
    Ok(permitted)
}
