//! Opaque-query bound for the private-service gateway (issue 749 P2b).
//!
//! Split from [`super::bounds`] so the whole rule set stays readable. The
//! query is bounded text and is never parsed: parsing it as anything would
//! be the first step towards treating a caller as able to name a target.

use super::BoundsRefusal;

/// Largest opaque query accepted for one service request.
pub const MAX_SERVICE_QUERY_BYTES: usize = 8 * 1024;

/// Accept an opaque, bounded query. It is never parsed.
pub fn validated_query(raw: Option<&str>) -> Result<Option<String>, BoundsRefusal> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() || raw.len() > MAX_SERVICE_QUERY_BYTES {
        return Err(BoundsRefusal::InvalidQuery);
    }
    if raw.chars().any(|character| character.is_control()) {
        return Err(BoundsRefusal::InvalidQuery);
    }
    Ok(Some(raw.to_string()))
}
