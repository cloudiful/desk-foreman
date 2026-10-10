//! Relay control frames (issue 749 P2a).
//!
//! Control frames are the only part of the tunnel that carries structure;
//! payloads travel as binary data frames. Every frame is tagged with a
//! `type` field and every payload struct denies unknown fields, so an
//! unexpected key is an error instead of being dropped on the floor.

use serde::{Deserialize, Serialize};

use super::RelayProtocolError;
use super::ident::RelayId;

/// Wire protocol version negotiated at tunnel open.
pub const RELAY_PROTOCOL_VERSION: &str = "df-env-http.v2";
/// Upper bound on one serialized control frame.
pub const MAX_CONTROL_JSON_BYTES: usize = 16 * 1024;
/// Upper bound on one whole transport message, control or binary.
pub const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024;

/// Start one relayed request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenFrame {
    /// Identifier this stream's data frames are prefixed with.
    pub stream_id: RelayId,
    /// Environment being served.
    pub environment_id: RelayId,
    /// Generation the caller believes it is addressing.
    pub generation: i64,
    /// Bounded request method.
    pub method: String,
    /// Bounded relative request path.
    pub path: String,
    /// Opaque raw query, never a target address.
    pub query: Option<String>,
    /// Header names explicitly permitted to cross the relay.
    pub allowed_headers: Vec<String>,
    /// Exact number of request body bytes that must follow.
    pub body_length: u64,
}

/// Response status and the headers permitted back to the caller.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseHeadFrame {
    /// Stream this head answers. Every response-direction frame carries its
    /// own identifier so multiplexed answers never rely on tunnel-singleton
    /// attribution.
    pub stream_id: RelayId,
    pub status: u16,
    pub allowed_headers: Vec<String>,
}

/// A refusal carrying a fixed code rather than diagnostic text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorFrame {
    /// Stream this refusal ends. Attributed like every other terminal frame
    /// so one stream's refusal can never be mistaken for another's.
    pub stream_id: RelayId,
    pub code: RelayErrorCode,
}

/// Payload for the frames that carry only their stream identifier.
///
/// These are structs rather than unit variants on purpose: a unit variant in
/// an internally tagged enum silently accepts and drops any extra keys,
/// which would let a peer smuggle data past a strict decoder. The identifier
/// is required on every one of these frames so request completion,
/// termination and cancellation are always attributable under multiplex.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamFrame {
    pub stream_id: RelayId,
}

/// Fixed refusal vocabulary. Closed on purpose: a peer that wants to report
/// something new needs a protocol version bump, not a free-form string.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayErrorCode {
    InvalidIdentifier,
    MalformedFrame,
    UnknownFrame,
    OutOfOrder,
    ControlTooLarge,
    MessageTooLarge,
    ChunkTooLarge,
    BodyTooLarge,
    BodyLengthMismatch,
    StreamLimit,
    QueueOverflow,
    NotFound,
    Conflict,
    Unauthorized,
    Cancelled,
    Timeout,
    UpstreamUnavailable,
    UpstreamError,
}

impl RelayErrorCode {
    /// Stable wire code. Kept in step with the serde `snake_case` form so a
    /// code can be compared at runtime without re-serializing.
    pub fn as_relay_code(self) -> &'static str {
        match self {
            Self::InvalidIdentifier => "invalid_identifier",
            Self::MalformedFrame => "malformed_frame",
            Self::UnknownFrame => "unknown_frame",
            Self::OutOfOrder => "out_of_order",
            Self::ControlTooLarge => "control_too_large",
            Self::MessageTooLarge => "message_too_large",
            Self::ChunkTooLarge => "chunk_too_large",
            Self::BodyTooLarge => "body_too_large",
            Self::BodyLengthMismatch => "body_length_mismatch",
            Self::StreamLimit => "stream_limit",
            Self::QueueOverflow => "queue_overflow",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Unauthorized => "unauthorized",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::UpstreamError => "upstream_error",
        }
    }
}

impl From<RelayProtocolError> for RelayErrorCode {
    fn from(error: RelayProtocolError) -> Self {
        match error {
            RelayProtocolError::InvalidIdentifier => Self::InvalidIdentifier,
            RelayProtocolError::MalformedFrame => Self::MalformedFrame,
            RelayProtocolError::UnknownFrame => Self::UnknownFrame,
            RelayProtocolError::OutOfOrder => Self::OutOfOrder,
            RelayProtocolError::ControlTooLarge => Self::ControlTooLarge,
            RelayProtocolError::MessageTooLarge => Self::MessageTooLarge,
            RelayProtocolError::ChunkTooLarge => Self::ChunkTooLarge,
            RelayProtocolError::BodyTooLarge => Self::BodyTooLarge,
            RelayProtocolError::BodyLengthMismatch => Self::BodyLengthMismatch,
            RelayProtocolError::StreamLimit => Self::StreamLimit,
            RelayProtocolError::QueueOverflow => Self::QueueOverflow,
        }
    }
}

/// One control frame.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayControl {
    Open(OpenFrame),
    RequestEnd(StreamFrame),
    ResponseHead(ResponseHeadFrame),
    End(StreamFrame),
    Error(ErrorFrame),
    Cancel(StreamFrame),
}

impl RelayControl {
    /// Identifier this frame belongs to, in either direction.
    ///
    /// Routing never guesses: every frame carries its own stream, including
    /// the terminal ones, so multiplexed streams correlate by reading the
    /// frame rather than by tunnel-singleton position.
    pub fn stream_id(&self) -> RelayId {
        match self {
            Self::Open(open) => open.stream_id,
            Self::RequestEnd(frame) => frame.stream_id,
            Self::ResponseHead(head) => head.stream_id,
            Self::End(frame) => frame.stream_id,
            Self::Error(frame) => frame.stream_id,
            Self::Cancel(frame) => frame.stream_id,
        }
    }
}

/// Serialize one control frame, refusing anything over the bound.
pub fn encode_control(control: &RelayControl) -> Result<String, RelayProtocolError> {
    let encoded = serde_json::to_string(control).map_err(|_| RelayProtocolError::MalformedFrame)?;
    if encoded.len() > MAX_CONTROL_JSON_BYTES {
        return Err(RelayProtocolError::ControlTooLarge);
    }
    Ok(encoded)
}

/// Parse one control frame, refusing anything over the bound first so an
/// oversized payload is never handed to the JSON parser.
pub fn decode_control(raw: &str) -> Result<RelayControl, RelayProtocolError> {
    if raw.len() > MAX_CONTROL_JSON_BYTES {
        return Err(RelayProtocolError::ControlTooLarge);
    }
    serde_json::from_str(raw).map_err(|_| RelayProtocolError::MalformedFrame)
}
