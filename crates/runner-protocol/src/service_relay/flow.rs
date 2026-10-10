//! Relay ordering, body-length and concurrency rules (issue 749 P2a).
//!
//! Frame shape alone is not enough to make a tunnel safe, so this module
//! holds the small state machines a receiver must run: which frame may
//! come next, whether the declared body length matches what arrived, and
//! whether another stream may be admitted at all.
//!
//! These are plain values with no transport or storage, so both ends apply
//! exactly the same rules and a later sub-phase only has to wire them to a
//! socket.

use super::RelayProtocolError;
use super::control::RelayControl;

/// Largest request or response body the relay will carry for one stream.
pub const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;
/// Concurrent streams admitted for a single environment.
pub const MAX_ACTIVE_STREAMS_PER_ENVIRONMENT: usize = 8;
/// Concurrent streams admitted for a single manager.
pub const MAX_ACTIVE_STREAMS_PER_MANAGER: usize = 32;
/// Buffered data frames allowed ahead of a stream's consumer.
pub const MAX_QUEUED_CHUNKS_PER_STREAM: usize = 32;

/// Where a stream currently is in its lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamPhase {
    /// Nothing accepted yet.
    New,
    /// Open accepted; request body may still arrive.
    Request,
    /// Request complete; response head expected.
    RequestComplete,
    /// Response head accepted; response body may still arrive.
    ResponseBody,
    /// Terminal: ended, refused, or cancelled.
    Closed,
}

/// Per-stream order validator.
///
/// Enforces that `Open` comes first, that the one-shot frames are not
/// repeated, and that nothing is accepted after a terminal frame. Without
/// this a peer could send a second `ResponseHead` to overwrite a status the
/// caller already acted on.
#[derive(Clone, Debug)]
pub struct StreamOrder {
    phase: StreamPhase,
}

impl Default for StreamOrder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamOrder {
    pub fn new() -> Self {
        Self {
            phase: StreamPhase::New,
        }
    }

    pub fn phase(&self) -> StreamPhase {
        self.phase
    }

    /// Accept one control frame, or refuse it.
    pub fn accept(&mut self, control: &RelayControl) -> Result<(), RelayProtocolError> {
        match control {
            RelayControl::Open(_) => self.step(StreamPhase::New, StreamPhase::Request),
            RelayControl::RequestEnd(_) => {
                self.step(StreamPhase::Request, StreamPhase::RequestComplete)
            }
            RelayControl::ResponseHead(_) => {
                self.step(StreamPhase::RequestComplete, StreamPhase::ResponseBody)
            }
            RelayControl::End(_) | RelayControl::Error(_) => self.step_any(
                &[
                    StreamPhase::Request,
                    StreamPhase::RequestComplete,
                    StreamPhase::ResponseBody,
                ],
                StreamPhase::Closed,
            ),
            RelayControl::Cancel(_) => self.step_any(
                &[
                    StreamPhase::Request,
                    StreamPhase::RequestComplete,
                    StreamPhase::ResponseBody,
                ],
                StreamPhase::Closed,
            ),
        }
    }

    fn step(&mut self, from: StreamPhase, to: StreamPhase) -> Result<(), RelayProtocolError> {
        if self.phase == from {
            self.phase = to;
            Ok(())
        } else {
            Err(RelayProtocolError::OutOfOrder)
        }
    }

    fn step_any(
        &mut self,
        from: &[StreamPhase],
        to: StreamPhase,
    ) -> Result<(), RelayProtocolError> {
        if from.contains(&self.phase) {
            self.phase = to;
            Ok(())
        } else {
            Err(RelayProtocolError::OutOfOrder)
        }
    }
}

/// Running check that arrived body bytes match what `Open` declared.
///
/// The declared length is authoritative: bytes beyond it are refused even
/// when they are individually within the global cap, and the stream cannot
/// be finished while short.
#[derive(Clone, Debug)]
pub struct BodyLengthGuard {
    declared: u64,
    received: u64,
}

impl BodyLengthGuard {
    pub fn new(declared: u64) -> Result<Self, RelayProtocolError> {
        if declared > MAX_BODY_BYTES {
            return Err(RelayProtocolError::BodyTooLarge);
        }
        Ok(Self {
            declared,
            received: 0,
        })
    }

    pub fn declared(&self) -> u64 {
        self.declared
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    /// Record `len` more body bytes.
    pub fn push(&mut self, len: usize) -> Result<(), RelayProtocolError> {
        let next = self
            .received
            .checked_add(len as u64)
            .ok_or(RelayProtocolError::BodyTooLarge)?;
        if next > self.declared {
            return Err(RelayProtocolError::BodyLengthMismatch);
        }
        if next > MAX_BODY_BYTES {
            return Err(RelayProtocolError::BodyTooLarge);
        }
        self.received = next;
        Ok(())
    }

    /// Accept the stream only when exactly the declared length arrived.
    pub fn finish(&self) -> Result<(), RelayProtocolError> {
        if self.received == self.declared {
            Ok(())
        } else {
            Err(RelayProtocolError::BodyLengthMismatch)
        }
    }
}

/// Admission decision for one more stream.
pub fn admit_stream(
    active_per_environment: usize,
    active_per_manager: usize,
) -> Result<(), RelayProtocolError> {
    if active_per_environment >= MAX_ACTIVE_STREAMS_PER_ENVIRONMENT
        || active_per_manager >= MAX_ACTIVE_STREAMS_PER_MANAGER
    {
        return Err(RelayProtocolError::StreamLimit);
    }
    Ok(())
}

/// Per-stream buffered-chunk depth, so a slow consumer applies
/// backpressure instead of growing an unbounded queue.
#[derive(Clone, Debug, Default)]
pub struct ChunkQueueDepth {
    depth: usize,
}

impl ChunkQueueDepth {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Buffer one chunk, refusing once the per-stream cap is reached.
    pub fn push(&mut self) -> Result<(), RelayProtocolError> {
        if self.depth >= MAX_QUEUED_CHUNKS_PER_STREAM {
            return Err(RelayProtocolError::QueueOverflow);
        }
        self.depth += 1;
        Ok(())
    }

    /// Release one buffered chunk.
    pub fn pop(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}
