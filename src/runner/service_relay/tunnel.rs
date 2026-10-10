//! Manager tunnel handle (issue 749 P2b).
//!
//! A tunnel is one authenticated WebSocket connection from a runner manager.
//! The handle is the registry's whole view of it: an outbound queue plus an
//! abort flag. Nothing here reads or writes a socket — the connection task
//! owns that — so admission and cancellation never block on transport.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use runner_protocol::service_relay::RelayId;
use tokio::sync::mpsc;

use super::frames::OutboundFrame;

/// Outbound frames buffered per tunnel.
///
/// Bounded so one slow socket cannot grow an unbounded queue for every stream
/// it carries. A full queue never drops a control frame: the tunnel is
/// aborted instead, which fails its streams with an explicit error.
pub const TUNNEL_OUTBOUND_CAPACITY: usize = 64;

/// Handle to one live manager tunnel.
#[derive(Clone)]
pub struct TunnelHandle {
    lease: RelayId,
    outbound: mpsc::Sender<OutboundFrame>,
    abort: Arc<AtomicBool>,
}

impl TunnelHandle {
    pub fn new(lease: RelayId, outbound: mpsc::Sender<OutboundFrame>) -> Self {
        Self {
            lease,
            outbound,
            abort: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Connection-scoped lease. Cleanup compares it so a slow connection
    /// tearing down late cannot remove the tunnel that replaced it.
    pub fn lease(&self) -> RelayId {
        self.lease
    }

    /// True once the connection task is gone or its outbound queue can no
    /// longer accept a frame.
    pub fn is_closed(&self) -> bool {
        self.outbound.is_closed()
    }

    pub fn is_aborted(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }

    /// Queue one frame without waiting.
    ///
    /// A full or closed queue is reported rather than absorbed: the caller
    /// turns it into an abort so the peer is never left believing a frame
    /// that will never be written was sent.
    pub fn try_send(&self, frame: OutboundFrame) -> bool {
        self.outbound.try_send(frame).is_ok()
    }

    /// Ask the connection task to tear the tunnel down.
    ///
    /// Synchronous by design so a stream ticket can guarantee its cancel
    /// reaches the agent side even while being dropped on a hot path.
    pub fn abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use runner_protocol::service_relay::RelayId;
    use uuid::Uuid;

    use super::{TUNNEL_OUTBOUND_CAPACITY, TunnelHandle};
    use crate::runner::service_relay::frames::OutboundFrame;

    fn handle(capacity: usize) -> (TunnelHandle, tokio::sync::mpsc::Receiver<OutboundFrame>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);
        (
            TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx),
            rx,
        )
    }

    fn frame() -> OutboundFrame {
        OutboundFrame::Text("{\"type\":\"request_end\"}".to_string())
    }

    #[tokio::test]
    async fn a_full_queue_is_reported_and_never_absorbed() {
        let (tunnel, mut rx) = handle(1);
        assert!(tunnel.try_send(frame()));
        assert!(!tunnel.try_send(frame()), "a full queue must be visible");
        assert_eq!(rx.recv().await, Some(frame()));
    }

    #[tokio::test]
    async fn a_dropped_receiver_closes_the_tunnel_handle() {
        let (tunnel, rx) = handle(TUNNEL_OUTBOUND_CAPACITY);
        assert!(!tunnel.is_closed());
        drop(rx);
        assert!(
            tunnel.is_closed(),
            "a gone connection is not a usable tunnel"
        );
    }

    #[tokio::test]
    async fn abort_is_visible_to_the_connection_task() {
        let (tunnel, _rx) = handle(TUNNEL_OUTBOUND_CAPACITY);
        assert!(!tunnel.is_aborted());
        tunnel.abort();
        assert!(tunnel.is_aborted());
    }
}
