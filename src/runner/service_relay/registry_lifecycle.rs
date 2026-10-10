//! Registry lifecycle cancellation (issue 749 P2d).
//!
//! Extracted from `registry.rs` so the registry never grows past its size
//! budget when multiplex rules change. Holds only the committed-mutation
//! cancellation paths — environment cancel, epoch check, and tunnel-loss
//! fail — which all share the ticket-weak-reference discipline.

use runner_protocol::service_relay::{RelayErrorCode, RelayId};
use uuid::Uuid;

use super::registry::RelayRegistry;

impl RelayRegistry {
    /// Cancel every stream for one environment after a committed lifecycle
    /// mutation.
    ///
    /// The epoch advances first, so a registration that races this call is
    /// refused afterwards: a later open captures the new epoch, and the
    /// database re-read still decides.
    pub fn cancel_environment(&self, environment_id: Uuid) -> usize {
        let mut state = self.lock();
        let epoch = state.environment_epochs.entry(environment_id).or_insert(0);
        *epoch = epoch.saturating_add(1);
        let victims: Vec<RelayId> = state
            .routes
            .iter()
            .filter(|(_, route)| route.environment_id == environment_id)
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

    /// True while no committed mutation has cancelled the environment since
    /// the given epoch was captured.
    pub fn epoch_matches(&self, environment_id: Uuid, epoch: u64) -> bool {
        self.lock()
            .environment_epochs
            .get(&environment_id)
            .copied()
            .unwrap_or(0)
            == epoch
    }

    /// Fail every stream carried by one manager's tunnel.
    ///
    /// The routes are left in place on purpose: only the ticket that still
    /// holds one may release its concurrency accounting, so removing the
    /// route here would leak the very slots this call exists to free.
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
