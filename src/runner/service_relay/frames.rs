//! Wire conversion between the frozen relay protocol and the tunnel
//! (issue 749 P2b).
//!
//! Two shapes leave this module and nothing else does: a frame to write to
//! the manager socket, and an event delivered to one waiting gateway call.
//! Keeping the conversion here means the tunnel loop and the HTTP handler
//! never touch the encoded protocol directly.

use axum::body::Bytes;
use runner_protocol::service_relay::{
    MAX_CHUNK_DATA_BYTES, RelayControl, RelayErrorCode, RelayId, RelayProtocolError,
    ResponseHeadFrame, decode_chunk, encode_chunk, encode_control,
};

/// One frame written to the manager's WebSocket.
///
/// Control frames are text, data frames are binary. The tunnel writer owns
/// the actual socket, so the direction a frame travels in is decided by
/// which side sends it, never by anything inside the frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundFrame {
    Text(String),
    Binary(Vec<u8>),
}

impl OutboundFrame {
    /// Encode one control frame within the frozen control bound.
    pub fn control(control: &RelayControl) -> Result<Self, RelayProtocolError> {
        Ok(Self::Text(encode_control(control)?))
    }

    /// Encode one data frame, refusing a payload the peer would reject.
    pub fn data(stream_id: RelayId, payload: &[u8]) -> Result<Self, RelayProtocolError> {
        Ok(Self::Binary(encode_chunk(stream_id, payload)?))
    }

    /// Split one oversized body slice into frames the protocol accepts.
    ///
    /// Splitting here rather than at the call site keeps the chunk bound in
    /// exactly one place; a caller cannot emit a frame its peer must refuse.
    pub fn data_chunks(
        stream_id: RelayId,
        payload: &[u8],
    ) -> Result<Vec<Self>, RelayProtocolError> {
        if payload.len() <= MAX_CHUNK_DATA_BYTES {
            return Ok(vec![Self::data(stream_id, payload)?]);
        }
        let mut frames = Vec::with_capacity(payload.len().div_ceil(MAX_CHUNK_DATA_BYTES));
        for slice in payload.chunks(MAX_CHUNK_DATA_BYTES) {
            frames.push(Self::data(stream_id, slice)?);
        }
        Ok(frames)
    }
}

/// One response event for a single admitted stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InboundEvent {
    /// Status plus the headers the agent is permitted to return.
    ResponseHead(ResponseHeadFrame),
    /// One incremental body slice. Delivery is per chunk so a long-lived
    /// response such as a server-sent event stream reaches the caller while
    /// the agent is still producing it.
    Data(Bytes),
    /// Clean end of the response body.
    End,
    /// Terminal refusal carrying a fixed relay code.
    Error(RelayErrorCode),
}

/// Outcome of handing one inbound frame to its stream.
///
/// A frame for a stream that no longer exists is discarded rather than
/// buffered; a frame for a stream whose caller stopped draining is
/// reported, because that is backpressure the relay must act on rather than
/// absorb.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamDelivery {
    Delivered,
    /// No such live stream on this tunnel.
    NoRoute,
    /// The per-stream queue is full.
    Backpressure,
}

/// Turn one inbound binary message into a data event for `stream_id`.
pub fn inbound_data(
    stream_id: RelayId,
    message: &[u8],
) -> Result<InboundEvent, RelayProtocolError> {
    let chunk = decode_chunk(message)?;
    if chunk.stream_id != stream_id {
        return Err(RelayProtocolError::InvalidIdentifier);
    }
    Ok(InboundEvent::Data(Bytes::copy_from_slice(chunk.data)))
}

/// Turn one inbound text message into the response event it carries.
///
/// `RequestEnd` and `Cancel` arrive from the agent side as acknowledgements
/// of the agent's own view of the stream; neither produces a response event,
/// so they are reported as having nothing to deliver rather than as a
/// malformed frame. Every frame already carries its own stream identifier,
/// so attribution happens by reading the frame, never by tunnel position.
pub fn inbound_control(control: &RelayControl) -> Option<InboundEvent> {
    match control {
        RelayControl::ResponseHead(head) => Some(InboundEvent::ResponseHead(head.clone())),
        RelayControl::End(_) => Some(InboundEvent::End),
        RelayControl::Error(frame) => Some(InboundEvent::Error(frame.code)),
        RelayControl::RequestEnd(_) | RelayControl::Cancel(_) | RelayControl::Open(_) => None,
    }
}

/// Identifier this inbound control frame belongs to, for lease-checked
/// routing. Data frames carry theirs as a binary prefix; control frames
/// carry theirs as a required `stream_id` field.
pub fn control_stream_id(control: &RelayControl) -> RelayId {
    control.stream_id()
}
