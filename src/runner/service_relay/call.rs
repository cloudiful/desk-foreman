//! One admitted relayed request (issue 749 P2b).
//!
//! A call is a plain `Arc` ticket: the gateway holds it, the request-body
//! forwarding task holds it, the response body stream holds it, and the
//! registry keeps only a `Weak` reference. Releasing the ticket therefore
//! releases the stream — its concurrency slot, its delivery route, and its
//! cancel to the agent side — through ordinary Rust drop, with no explicit
//! teardown call that a panicking or abandoned path could skip.
//!
//! Terminal cancellation travels on a watch channel rather than on the
//! inbound queue. A stream is cancelled by another task, and a caller that is
//! not draining its response would otherwise never learn that its stream was
//! stopped underneath it.

use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

use runner_protocol::service_relay::{
    BodyLengthGuard, MAX_QUEUED_CHUNKS_PER_STREAM, OpenFrame, RelayControl, RelayErrorCode,
    RelayId, StreamFrame,
};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use super::{
    errors::RelayCallError,
    frames::{InboundEvent, OutboundFrame},
    registry::RelayRegistry,
    tunnel::TunnelHandle,
};

/// Per-stream inbound delivery depth. Bounded so a caller that stops reading
/// applies backpressure to the tunnel instead of buffering a whole response.
pub const STREAM_QUEUE_CAPACITY: usize = MAX_QUEUED_CHUNKS_PER_STREAM;

/// Shared ticket state. Dropped when the last [`RelayCall`] clone goes away.
pub(crate) struct CallInner {
    registry: Weak<RelayRegistry>,
    tunnel: TunnelHandle,
    stream_id: RelayId,
    environment_id: Uuid,
    manager_id: i64,
    epoch: u64,
    events: mpsc::Sender<InboundEvent>,
    response: Mutex<Option<mpsc::Receiver<InboundEvent>>>,
    request: Mutex<BodyLengthGuard>,
    request_ended: AtomicBool,
    cancelled: AtomicBool,
    terminal: watch::Sender<Option<RelayErrorCode>>,
}

/// Handle to one relayed request.
#[derive(Clone)]
pub struct RelayCall {
    pub(crate) inner: Arc<CallInner>,
}

impl RelayCall {
    pub(crate) fn new(
        registry: &Arc<RelayRegistry>,
        tunnel: TunnelHandle,
        stream_id: RelayId,
        environment_id: Uuid,
        manager_id: i64,
        epoch: u64,
        events: mpsc::Sender<InboundEvent>,
        response: mpsc::Receiver<InboundEvent>,
    ) -> Result<Self, RelayCallError> {
        let (terminal, _) = watch::channel(None);
        Ok(Self {
            inner: Arc::new(CallInner {
                registry: Arc::downgrade(registry),
                tunnel,
                stream_id,
                environment_id,
                manager_id,
                epoch,
                events,
                response: Mutex::new(Some(response)),
                request: Mutex::new(
                    BodyLengthGuard::new(0)
                        .expect("a zero-length declared body is always admissible"),
                ),
                request_ended: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                terminal,
            }),
        })
    }

    /// Bind the length this stream promised before anything is opened.
    ///
    /// Set once, before the opening frame: the agent learns the declared
    /// length from that frame, so changing it afterwards would desynchronise
    /// the two ends.
    pub fn declare_body_length(&self, declared: u64) -> Result<(), RelayCallError> {
        if self.inner.request_ended.load(Ordering::SeqCst) {
            return Err(RelayCallError::TunnelUnavailable);
        }
        let guard = BodyLengthGuard::new(declared).map_err(RelayCallError::Protocol)?;
        *self
            .inner
            .request
            .lock()
            .expect("request body accounting is not poisoned") = guard;
        Ok(())
    }

    pub fn stream_id(&self) -> RelayId {
        self.inner.stream_id
    }

    pub fn environment_id(&self) -> Uuid {
        self.inner.environment_id
    }

    pub fn manager_id(&self) -> i64 {
        self.inner.manager_id
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Take the response side of the stream.
    ///
    /// Taken once: a second caller gets `None` rather than a second reader,
    /// so one stream can never feed two response bodies.
    pub fn take_response(&self) -> Option<mpsc::Receiver<InboundEvent>> {
        self.inner
            .response
            .lock()
            .expect("response slot is not poisoned")
            .take()
    }

    /// Signal that fires when a committed mutation or a lost tunnel stops
    /// this stream.
    pub fn terminal_signal(&self) -> watch::Receiver<Option<RelayErrorCode>> {
        self.inner.terminal.subscribe()
    }

    /// True while no committed lifecycle mutation has cancelled this
    /// environment since the stream was admitted.
    pub fn epoch_is_current(&self) -> bool {
        self.inner.registry.upgrade().is_some_and(|registry| {
            registry.epoch_matches(self.inner.environment_id, self.inner.epoch)
        })
    }

    /// Send the opening frame. Nothing else may precede it.
    pub fn send_open(&self, open: &OpenFrame) -> Result<(), RelayCallError> {
        self.send_control(&RelayControl::Open(open.clone()))
    }

    /// Forward one slice of the caller's request body.
    ///
    /// The frozen protocol's own body-length guard is the accounting, so a
    /// stream can neither exceed nor silently shorten what it declared.
    pub fn push_request_body(&self, payload: &[u8]) -> Result<(), RelayCallError> {
        if self.is_cancelled() {
            return Err(RelayCallError::TunnelUnavailable);
        }
        {
            let mut guard = self
                .inner
                .request
                .lock()
                .expect("request body accounting is not poisoned");
            guard
                .push(payload.len())
                .map_err(RelayCallError::Protocol)?;
        }
        let frames = OutboundFrame::data_chunks(self.inner.stream_id, payload)
            .map_err(RelayCallError::Protocol)?;
        self.send_frames(frames)
    }

    /// Declare the request complete.
    ///
    /// Refused unless the declared length and the bytes actually forwarded
    /// agree, so a dropped client body can never reach the agent described as
    /// a complete request.
    pub fn end_request(&self) -> Result<(), RelayCallError> {
        if self
            .inner
            .request_ended
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(RelayCallError::TunnelUnavailable);
        }
        {
            let guard = self
                .inner
                .request
                .lock()
                .expect("request body accounting is not poisoned");
            guard.finish().map_err(RelayCallError::Protocol)?;
        }
        self.send_control(&RelayControl::RequestEnd(StreamFrame {
            stream_id: self.inner.stream_id,
        }))
    }

    /// Cancel the stream. Idempotent, and never silently lost: a full
    /// outbound queue aborts the tunnel rather than dropping the frame.
    pub fn cancel(&self) {
        self.inner.cancel_with(RelayErrorCode::Cancelled);
    }

    fn send_control(&self, control: &RelayControl) -> Result<(), RelayCallError> {
        let frame = OutboundFrame::control(control).map_err(RelayCallError::Protocol)?;
        self.send_frames(vec![frame])
    }

    fn send_frames(&self, frames: Vec<OutboundFrame>) -> Result<(), RelayCallError> {
        if self.inner.tunnel.is_aborted() || self.inner.tunnel.is_closed() {
            return Err(RelayCallError::TunnelUnavailable);
        }
        for frame in frames {
            if !self.inner.tunnel.try_send(frame) {
                return Err(RelayCallError::TunnelUnavailable);
            }
        }
        Ok(())
    }
}

impl CallInner {
    /// Deliver one inbound event to the waiting caller.
    ///
    /// Reported rather than absorbed: a stream whose caller stopped draining
    /// is dropped here instead of buffered.
    pub(crate) fn deliver(&self, event: InboundEvent) -> bool {
        self.events.try_send(event).is_ok()
    }

    /// Cancel the stream. Idempotent, and never silently lost: a full
    /// outbound queue aborts the tunnel rather than dropping the frame.
    ///
    /// Returns whether this call is the one that cancelled it, so a caller
    /// can report how many streams a committed mutation actually stopped.
    pub(crate) fn cancel(&self) -> bool {
        self.cancel_with(RelayErrorCode::Cancelled)
    }

    /// Cancel the stream and tell the caller why it stopped.
    ///
    /// The outbound cancel is sent at most once, so a cancelled stream is
    /// never described twice to the agent side. Returns true the first time
    /// only.
    pub(crate) fn cancel_with(&self, code: RelayErrorCode) -> bool {
        let first = !self.cancelled.swap(true, Ordering::SeqCst);
        if first
            && let Ok(frame) = OutboundFrame::control(&RelayControl::Cancel(StreamFrame {
                stream_id: self.stream_id,
            }))
            && !self.tunnel.try_send(frame)
        {
            self.tunnel.abort();
        }
        self.terminal.send_replace(Some(code));
        first
    }
}

impl Drop for CallInner {
    fn drop(&mut self) {
        // A ticket that was neither finished nor cancelled releases its
        // stream on the agent side. This runs on every exit path, including
        // an abandoned handler, because it is the ticket's own drop.
        if !self.request_ended.load(Ordering::SeqCst)
            && !self.inner_cancelled()
            && let Ok(frame) = OutboundFrame::control(&RelayControl::Cancel(StreamFrame {
                stream_id: self.stream_id,
            }))
            && !self.tunnel.try_send(frame)
        {
            self.tunnel.abort();
        }
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(registry) = self.registry.upgrade() {
            registry.release_stream(self.manager_id, self.environment_id, self.stream_id);
        }
    }
}

impl CallInner {
    fn inner_cancelled(&self) -> bool {
        self.cancelled.swap(true, Ordering::SeqCst)
    }
}
