//! Bounded recently-closed stream identifiers (issue 749 P2d).
//!
//! Split from the registry so the registry itself never grows past its size
//! budget. A closed identifier is remembered just long enough to refuse its
//! immediate reuse and to discard its delayed frames without touching an
//! unrelated live stream. The set is bounded: the oldest entry is evicted
//! once the cap is reached, so memory never grows with connection lifetime.
//!
//! The cap is deliberately small — connection-scoped reuse is refused while
//! the identifier is still remembered, and an evicted identifier that
//! reappears is treated as a fresh duplicate check against the live routes
//! (refused if live, admitted if fully gone). No unbounded identifier log.

use std::collections::VecDeque;

use runner_protocol::service_relay::RelayId;

/// How many recently closed stream identifiers are remembered.
///
/// 128 identifiers occupy ~2KiB and comfortably cover the in-flight window
/// (at most 32 live per manager) plus reconnect races, without ever growing
/// with the connection's total stream count.
pub const CLOSED_STREAM_CAPACITY: usize = 128;

/// Bounded FIFO of recently closed stream identifiers.
#[derive(Clone, Debug)]
pub struct ClosedStreamIds {
    ids: VecDeque<RelayId>,
}

impl Default for ClosedStreamIds {
    fn default() -> Self {
        Self::new()
    }
}

impl ClosedStreamIds {
    pub fn new() -> Self {
        Self {
            ids: VecDeque::with_capacity(CLOSED_STREAM_CAPACITY),
        }
    }

    /// True while this identifier is still remembered as recently closed.
    pub fn contains(&self, stream_id: RelayId) -> bool {
        self.ids.contains(&stream_id)
    }

    /// Remember one closed identifier, evicting the oldest when full.
    ///
    /// Idempotent for the same identifier: a double release never occupies
    /// two slots, so quota accounting stays exact.
    pub fn insert(&mut self, stream_id: RelayId) {
        if self.ids.contains(&stream_id) {
            return;
        }
        if self.ids.len() >= CLOSED_STREAM_CAPACITY {
            self.ids.pop_front();
        }
        self.ids.push_back(stream_id);
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{CLOSED_STREAM_CAPACITY, ClosedStreamIds};
    use runner_protocol::service_relay::RelayId;

    fn id() -> RelayId {
        RelayId::from_bytes(Uuid::new_v4().into_bytes())
    }

    #[test]
    fn a_closed_id_is_remembered_and_idempotent() {
        let mut closed = ClosedStreamIds::new();
        let stream = id();
        assert!(!closed.contains(stream));
        closed.insert(stream);
        assert!(closed.contains(stream));
        closed.insert(stream);
        assert_eq!(closed.len(), 1);
    }

    #[test]
    fn the_oldest_id_is_evicted_once_full() {
        let mut closed = ClosedStreamIds::new();
        let first = id();
        closed.insert(first);
        for _ in 1..=CLOSED_STREAM_CAPACITY {
            closed.insert(id());
        }
        assert_eq!(closed.len(), CLOSED_STREAM_CAPACITY);
        assert!(
            !closed.contains(first),
            "bounded memory evicts the oldest closed id"
        );
    }
}
