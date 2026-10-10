//! Declared request-body length (issue 749 P2b).
//!
//! Split from [`super::bounds`] so the whole rule set stays readable. The
//! length is required rather than inferred: a relay cannot promise a
//! byte-exact body to the agent side without it, and inferring from a
//! chunked body would mean guessing how many bytes will arrive.

use axum::http::{HeaderMap, header};

use super::BoundsRefusal;

/// The exact request body length, bounded before anything is opened.
///
/// The three failures are kept apart on purpose. A missing, unparseable or
/// non-numeric `Content-Length` is a malformed request (400), while a
/// well-formed length that exceeds the relay bound is a body the relay
/// refuses to carry (413), which is the status the gateway publishes.
pub fn declared_body_length(headers: &HeaderMap) -> Result<u64, BoundsRefusal> {
    let raw = headers
        .get(header::CONTENT_LENGTH)
        .ok_or(BoundsRefusal::InvalidBodyLength)?
        .to_str()
        .map_err(|_| BoundsRefusal::InvalidBodyLength)?;
    let declared: u64 = raw.parse().map_err(|_| BoundsRefusal::InvalidBodyLength)?;
    if declared > runner_protocol::service_relay::MAX_BODY_BYTES {
        return Err(BoundsRefusal::BodyTooLarge);
    }
    Ok(declared)
}
