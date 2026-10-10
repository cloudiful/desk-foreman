//! Binary relay data frames (issue 749 P2a).
//!
//! A data frame is the 16-byte stream id followed by at most
//! [`MAX_CHUNK_DATA_BYTES`] of payload. The direction of travel is implied
//! by which side sent it, not by anything in the frame, so the header stays
//! exactly the identifier and nothing else.

use super::RelayProtocolError;
use super::ident::{RELAY_ID_BYTES, RelayId};

/// Upper bound on the payload carried by one data frame.
pub const MAX_CHUNK_DATA_BYTES: usize = 32 * 1024;

/// A decoded data frame borrowing its payload from the transport buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Chunk<'a> {
    pub stream_id: RelayId,
    pub data: &'a [u8],
}

/// Build one data frame. The payload is bounded rather than split here so a
/// caller cannot quietly emit a frame the peer will reject.
pub fn encode_chunk(stream_id: RelayId, data: &[u8]) -> Result<Vec<u8>, RelayProtocolError> {
    if data.len() > MAX_CHUNK_DATA_BYTES {
        return Err(RelayProtocolError::ChunkTooLarge);
    }
    let mut frame = Vec::with_capacity(RELAY_ID_BYTES + data.len());
    frame.extend_from_slice(stream_id.as_bytes());
    frame.extend_from_slice(data);
    Ok(frame)
}

/// Parse one data frame. The payload borrows the input, so a large
/// transport read is never copied twice.
pub fn decode_chunk(frame: &[u8]) -> Result<Chunk<'_>, RelayProtocolError> {
    if frame.len() < RELAY_ID_BYTES {
        return Err(RelayProtocolError::MalformedFrame);
    }
    if frame.len() > RELAY_ID_BYTES + MAX_CHUNK_DATA_BYTES {
        return Err(RelayProtocolError::ChunkTooLarge);
    }
    let mut id = [0u8; RELAY_ID_BYTES];
    id.copy_from_slice(&frame[..RELAY_ID_BYTES]);
    // The identifier travels as raw bytes here, so there is nothing to
    // re-validate; its canonical form was already validated on the control
    // frame that opened the stream.
    Ok(Chunk {
        stream_id: RelayId::from_bytes(id),
        data: &frame[RELAY_ID_BYTES..],
    })
}
