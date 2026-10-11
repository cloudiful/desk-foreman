//! Registry lifecycle cancellation (issue 749 P2d).
//!
//! A committed mutation fences only the generations it supersedes. The
//! per-environment high-water mark also makes delayed cancellation callbacks
//! safe for streams admitted at a successor generation.

use runner_protocol::service_relay::{RelayErrorCode, RelayId};
use uuid::Uuid;

use super::registry::RelayRegistry;

impl RelayRegistry {
    /// Cancel streams through one committed authorization generation.
    ///
    /// The monotonic cutoff is recorded before routes are inspected. Repeating
    /// an older callback cannot move it backward or affect successor streams.
    pub fn cancel_environment_through(&self, environment_id: Uuid, generation: i64) -> usize {
        let mut state = self.lock();
        let cutoff = state
            .cancelled_through_generation
            .entry(environment_id)
            .and_modify(|current| *current = (*current).max(generation))
            .or_insert(generation);
        let cutoff = *cutoff;
        let victims: Vec<RelayId> = state
            .routes
            .iter()
            .filter(|(_, route)| {
                route.environment_id == environment_id && route.generation <= cutoff
            })
            .map(|(stream_id, _)| *stream_id)
            .collect();
        let mut cancelled = 0usize;
        for stream_id in victims {
            let inner = state
                .routes
                .get(&stream_id)
                .and_then(|route| route.call.upgrade());
            match inner {
                Some(inner) => {
                    if inner.cancel() {
                        cancelled += 1;
                    }
                }
                None => {
                    state.routes.remove(&stream_id);
                }
            }
        }
        cancelled
    }

    /// True while a stream's generation remains above the committed cutoff.
    pub fn generation_is_current(&self, environment_id: Uuid, generation: i64) -> bool {
        self.lock()
            .cancelled_through_generation
            .get(&environment_id)
            .is_none_or(|cutoff| generation > *cutoff)
    }

    /// Fail every stream carried by one manager's tunnel.
    ///
    /// The routes are left in place on purpose: only the ticket that still
    /// holds one may release its concurrency accounting, so removing the route
    /// here would leak the very slots this call exists to free.
    pub fn fail_manager_streams(&self, manager_id: i64) {
        let calls = {
            let state = self.lock();
            let routes: Vec<RelayId> = state
                .routes
                .iter()
                .filter(|(_, route)| route.manager_id == manager_id)
                .map(|(stream_id, _)| *stream_id)
                .collect();
            routes
                .into_iter()
                .filter_map(|stream_id| {
                    state
                        .routes
                        .get(&stream_id)
                        .and_then(|route| route.call.upgrade())
                })
                .collect::<Vec<_>>()
        };
        for call in calls {
            call.cancel_with(RelayErrorCode::UpstreamUnavailable);
        }
    }
}
