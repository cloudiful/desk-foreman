//! Per-tunnel stream tables (issue 749 P2d).
//!
//! Extracted from [`super::dispatch`] so neither file exceeds the size
//! budget. Holds only the assembling/serving maps and the bounded
//! recently-closed set with its exact accounting; routing decisions stay in
//! the dispatcher.

use std::collections::{HashMap, VecDeque};

use runner_protocol::service_relay::RelayId;

use super::streams::StreamSlot;

/// Recently closed identifiers remembered to refuse reuse and isolate
/// delayed frames. Bounded so memory never grows with connection lifetime.
const CLOSED_CAPACITY: usize = 128;

/// Streams in flight on one tunnel.
///
/// Assembling bodies and serving responses both multiplex by identifier,
/// bounded only by the fixed quotas. Every removal path also releases its
/// quota slot and remembers the identifier in the bounded closed set, so no
/// finished stream leaks a pending count and no delayed frame can kill an
/// unrelated live stream.
pub(crate) struct ConnState {
    pub(crate) assembling: HashMap<RelayId, StreamSlot>,
    pub(crate) serving: HashMap<RelayId, (RelayId, tokio::task::AbortHandle)>,
    pub(crate) closed: VecDeque<RelayId>,
}

impl ConnState {
    pub(crate) fn new() -> Self {
        Self {
            assembling: HashMap::new(),
            serving: HashMap::new(),
            closed: VecDeque::with_capacity(CLOSED_CAPACITY),
        }
    }

    pub(crate) fn is_closed(&self, id: RelayId) -> bool {
        self.closed.contains(&id)
    }

    pub(crate) fn remember_closed(&mut self, id: RelayId) {
        if self.closed.contains(&id) {
            return;
        }
        if self.closed.len() >= CLOSED_CAPACITY {
            self.closed.pop_front();
        }
        self.closed.push_back(id);
    }
}
