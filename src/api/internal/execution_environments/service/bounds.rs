//! Request bounds for the private-service gateway (issue 749 P2b).
//!
//! Thin module entry: everything a caller can influence is bounded and
//! checked before any database read and long before anything is opened on a
//! tunnel. The rules are pure functions over strings so each one is directly
//! testable and none of them can grow a hidden dependency on transport state.
//!
//! Each rule lives in its own sibling module so the whole set stays
//! readable: [`path`] and [`query`] treat the request target as opaque text,
//! [`headers`] owns both directions of the relay header entry, and [`body`]
//! owns the declared length. Nothing here changes behaviour; the split is
//! organization only.
//!
//! The path is treated as an opaque relative reference, never as a URL. A
//! gateway that could be talked into building an absolute URL would let a
//! caller choose the target, which is exactly what this relay exists to
//! prevent.

pub mod body;
pub mod headers;
pub mod path;
pub mod query;

use axum::http::{HeaderMap, Method};

pub use body::declared_body_length;
pub use headers::{
    MAX_SERVICE_HEADER_VALUE_BYTES, header_entry, parse_header_entry, permitted_request_headers,
};
pub use path::validated_path;
pub use query::validated_query;

/// Methods the gateway relays. Anything else is refused before admission.
pub const RELAYED_METHODS: [Method; 7] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
    Method::HEAD,
    Method::OPTIONS,
];

/// One validated, bounded request description.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceRequestBounds {
    /// Bounded relative path, without a leading slash.
    pub path: String,
    /// Opaque query, forwarded verbatim and never parsed as a target.
    pub query: Option<String>,
    /// Request header values permitted to cross the relay.
    pub allowed_headers: Vec<(String, String)>,
    /// Exact declared body length.
    pub body_length: u64,
}

/// Why a request was refused before admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundsRefusal {
    /// The method is not one this gateway relays.
    UnsupportedMethod,
    /// The path is empty, absolute, escaping, or otherwise not a plain
    /// relative reference.
    InvalidPath,
    /// The query is oversized or carries a control character.
    InvalidQuery,
    /// A forwarded header value is oversized or malformed.
    InvalidHeader,
    /// The declared body length is missing or malformed.
    InvalidBodyLength,
    /// The declared body length is well formed but over the relay bound.
    BodyTooLarge,
}

/// Validate one request against every bound the relay applies.
///
/// Returns the bounded description rather than a boolean so the caller
/// cannot later forward something it did not check.
pub fn bounded_service_request(
    method: &Method,
    raw_path: &str,
    raw_query: Option<&str>,
    headers: &HeaderMap,
) -> Result<ServiceRequestBounds, BoundsRefusal> {
    if !RELAYED_METHODS.contains(method) {
        return Err(BoundsRefusal::UnsupportedMethod);
    }
    let path = validated_path(raw_path)?;
    let query = validated_query(raw_query)?;
    let allowed_headers = permitted_request_headers(headers)?;
    let body_length = declared_body_length(headers)?;
    Ok(ServiceRequestBounds {
        path,
        query,
        allowed_headers,
        body_length,
    })
}

#[cfg(test)]
mod tests;
