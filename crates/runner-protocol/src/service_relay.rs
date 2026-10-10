//! Bounded relay wire contract (issue 749 P2a).
//!
//! Shared framing between the control plane and one execution-environment
//! relay tunnel. This module is deliberately transport-agnostic: it defines
//! the bytes and the rules that both ends must agree on, and nothing about
//! how they are carried. A later sub-phase supplies the WebSocket
//! transport and the HTTP forwarding that consume it.
//!
//! Two frame kinds travel over the same connection:
//!
//! - **Control** frames are canonical JSON, tagged by a `type` field, and
//!   bounded to [`MAX_CONTROL_JSON_BYTES`].
//! - **Data** frames are binary and start with the 16-byte stream id,
//!   followed by at most [`MAX_CHUNK_DATA_BYTES`] of payload.
//!
//! Every bound in this module is a hard limit rather than a hint: encoding
//! returns an error instead of producing an oversized frame, and decoding
//! refuses before it allocates. Unknown fields, unknown variants, and
//! malformed identifiers are rejected rather than defaulted, because a
//! relay that guesses on the wire can silently cross a tenant boundary.

pub mod chunk;
pub mod control;
pub mod flow;
#[cfg(test)]
mod flow_tests;
mod ident;
#[cfg(test)]
mod tests;

pub use chunk::{Chunk, MAX_CHUNK_DATA_BYTES, decode_chunk, encode_chunk};
pub use control::{
    ErrorFrame, MAX_CONTROL_JSON_BYTES, MAX_WS_MESSAGE_BYTES, OpenFrame, RELAY_PROTOCOL_VERSION,
    RelayControl, RelayErrorCode, ResponseHeadFrame, StreamFrame, decode_control, encode_control,
};
pub use flow::{
    BodyLengthGuard, ChunkQueueDepth, MAX_ACTIVE_STREAMS_PER_ENVIRONMENT,
    MAX_ACTIVE_STREAMS_PER_MANAGER, MAX_BODY_BYTES, MAX_QUEUED_CHUNKS_PER_STREAM, StreamOrder,
    StreamPhase, admit_stream,
};
pub use ident::{RELAY_ID_BYTES, RelayId, parse_relay_id};

/// Reason a frame was refused. Every variant maps to a fixed wire code so a
/// peer never has to parse free-form diagnostic text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayProtocolError {
    /// A stream or environment id was not a canonical identifier.
    InvalidIdentifier,
    /// The frame was not decodable as its declared kind.
    MalformedFrame,
    /// The frame kind is not part of this protocol.
    UnknownFrame,
    /// The frame arrived out of order, or repeated a one-shot frame.
    OutOfOrder,
    /// The control frame exceeded [`MAX_CONTROL_JSON_BYTES`].
    ControlTooLarge,
    /// The whole message exceeded [`MAX_WS_MESSAGE_BYTES`].
    MessageTooLarge,
    /// A data chunk exceeded [`MAX_CHUNK_DATA_BYTES`].
    ChunkTooLarge,
    /// The body exceeded [`MAX_BODY_BYTES`].
    BodyTooLarge,
    /// The declared body length did not match the bytes actually sent.
    BodyLengthMismatch,
    /// Opening this stream would exceed a concurrency cap.
    StreamLimit,
    /// A per-stream chunk queue exceeded [`MAX_QUEUED_CHUNKS_PER_STREAM`].
    QueueOverflow,
}

impl RelayProtocolError {
    /// Stable wire code. These strings are part of the protocol and must
    /// not be reworded without a version bump.
    pub fn code(self) -> &'static str {
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
        }
    }
}

impl std::fmt::Display for RelayProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for RelayProtocolError {}
