//! The relay registry (issue 749 P2d).
//!
//! One registry holds every live manager tunnel and every admitted stream.
//! It is deliberately the *least* authoritative thing in the path: it decides
//! whether a stream may be opened, it never decides whether one may serve.
//! The database owns that, and a stream is re-checked against it after it is
//! registered here.
//!
//! State lives behind a plain `std::sync::Mutex` because every critical
//! section is short and holds nothing across an await. That is also what lets
//! a stream ticket release itself from its own `Drop`: no async lock can be
//! taken from a destructor.
//!
//! The registry holds only `Weak` references to live tickets, so a stream is
//! released by ordinary reference counting rather than by a teardown call
//! that an abandoned handler could skip.
//!
//! Multiplex (`df-env-http.v2`): every frame in both directions carries its
//! own stream identifier, so tunnels admit many concurrent streams up to the
//! fixed quotas. Routing verifies the authenticated manager *and* its
//! connection lease against the stored route, so a forged cross-manager
//! identifier can never affect another tenant's stream. Recently closed
//! identifiers are remembered in a bounded set to refuse reuse and to
//! discard delayed frames without touching live streams.

use std::sync::{Arc, Mutex};

use runner_protocol::service_relay::RelayId;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    admit_check::{Admission, check_open},
    call::{RelayCall, STREAM_QUEUE_CAPACITY},
    errors::RelayAdmissionError,
    frames::{InboundEvent, StreamDelivery},
    registry_state::{
        RegistryState, StreamRoute, TunnelSlot, decrement_environment, decrement_manager,
        prune_dead_routes,
    },
    tunnel::TunnelHandle,
};

/// What one caller wants to open.
///
/// The fence facts come from the database, never from the caller; this
/// carries only what the stream needs to exist and the length it promised.
#[derive(Clone, Debug)]
pub struct StreamRequest {
    pub stream_id: RelayId,
    pub environment_id: Uuid,
    pub manager_id: i64,
    pub declared_body_length: u64,
}

/// Owner-scoped reverse-relay registry.
pub struct RelayRegistry {
    state: Mutex<RegistryState>,
}

impl RelayRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RegistryState::default()),
        })
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .expect("relay registry state is not poisoned")
    }

    /// Admit one manager tunnel, or refuse because one is already live.
    ///
    /// A tunnel whose connection task is gone is replaced rather than
    /// refused: the entry is a cache of a connection, and the connection is
    /// the authority on whether it is still usable.
    pub fn attach_tunnel(
        &self,
        manager_id: i64,
        handle: TunnelHandle,
    ) -> Result<RelayId, RelayAdmissionError> {
        let mut state = self.lock();
        prune_dead_routes(&mut state);
        if let Some(existing) = state.tunnels.get(&manager_id)
            && !existing.handle.is_closed()
        {
            return Err(RelayAdmissionError::TunnelConflict);
        }
        let lease = handle.lease();
        state
            .tunnels
            .insert(manager_id, TunnelSlot { lease, handle });
        Ok(lease)
    }

    /// Remove one tunnel, but only the one named by `lease`.
    ///
    /// A connection tearing down after it was replaced must not remove the
    /// replacement, so the lease is compared rather than trusted.
    pub fn detach_tunnel(&self, manager_id: i64, lease: RelayId) {
        let removed = {
            let mut state = self.lock();
            prune_dead_routes(&mut state);
            match state.tunnels.get(&manager_id) {
                Some(slot) if slot.lease == lease => state.tunnels.remove(&manager_id).is_some(),
                _ => false,
            }
        };
        if removed {
            self.fail_manager_streams(manager_id);
        }
    }

    /// The live tunnel for one manager, if any.
    pub fn tunnel_for(&self, manager_id: i64) -> Option<TunnelHandle> {
        let mut state = self.lock();
        prune_dead_routes(&mut state);
        match state.tunnels.get(&manager_id) {
            Some(slot) if !slot.handle.is_closed() => Some(slot.handle.clone()),
            Some(_) => {
                state.tunnels.remove(&manager_id);
                None
            }
            None => None,
        }
    }

    /// Admit one stream against the manager's live tunnel.
    ///
    /// Takes `&Arc<Self>` because the admitted ticket keeps a `Weak` back to
    /// this registry so it can release itself; the downgrade keeps the cycle
    /// broken without any unsafe ownership. Synchronous checks live in
    /// `admit_check` so this file stays wiring-only.
    ///
    /// Check, ticket mint, and route reservation happen inside **one**
    /// critical section. The queue channels are minted before the lock so
    /// the section stays short, but nothing about the admission decision is
    /// read outside it: a stale count written back after a second lock would
    /// let two racing opens both observe the same pre-admission number and
    /// both be admitted past the cap.
    pub fn open_stream(
        self: &Arc<Self>,
        request: StreamRequest,
    ) -> Result<RelayCall, RelayAdmissionError> {
        let (events, response) = mpsc::channel(STREAM_QUEUE_CAPACITY);
        let mut state = self.lock();
        let Admission {
            tunnel,
            lease,
            epoch,
            environment_streams,
            manager_streams,
        } = check_open(&mut state, &request)?;
        let call = RelayCall::new(
            self,
            tunnel,
            request.stream_id,
            request.environment_id,
            request.manager_id,
            epoch,
            events,
            response,
        )
        .map_err(|_| RelayAdmissionError::BodyTooLarge)?;
        // A duplicate identifier is refused by `check_open` above, so this
        // insert can never overwrite an existing route.
        let previous = state.routes.insert(
            request.stream_id,
            StreamRoute {
                manager_id: request.manager_id,
                lease,
                environment_id: request.environment_id,
                call: Arc::downgrade(&call.inner),
            },
        );
        debug_assert!(
            previous.is_none(),
            "check_open refuses a live identifier, so an insert must not replace a route"
        );
        state
            .environment_streams
            .insert(request.environment_id, environment_streams + 1);
        state
            .manager_streams
            .insert(request.manager_id, manager_streams + 1);
        Ok(call)
    }

    /// Deliver one inbound frame to its stream.
    ///
    /// The authenticated manager *and* its connection lease must both match
    /// the stored route: a forged identifier from another manager, a stale
    /// lease, or a recently closed stream is discarded (`NoRoute`) rather
    /// than delivered or buffered. A live stream whose caller stopped
    /// draining reports backpressure.
    pub fn deliver(
        &self,
        manager_id: i64,
        lease: RelayId,
        stream_id: RelayId,
        event: InboundEvent,
    ) -> StreamDelivery {
        let inner = {
            let state = self.lock();
            match state.routes.get(&stream_id) {
                Some(route) if route.manager_id == manager_id && route.lease == lease => {
                    route.call.upgrade()
                }
                _ => None,
            }
        };
        match inner {
            Some(inner) if inner.deliver(event) => StreamDelivery::Delivered,
            Some(_) => StreamDelivery::Backpressure,
            None => StreamDelivery::NoRoute,
        }
    }

    /// Release the registry bookkeeping for one finished stream.
    ///
    /// Called from the ticket's own drop, so a stream cannot keep a
    /// concurrency slot after its caller is gone. The identifier joins the
    /// bounded recently-closed set exactly once, however the stream ends.
    pub fn release_stream(&self, manager_id: i64, environment_id: Uuid, stream_id: RelayId) {
        let mut state = self.lock();
        if state.routes.remove(&stream_id).is_none() {
            return;
        }
        state.closed.insert(stream_id);
        decrement_environment(&mut state, environment_id);
        decrement_manager(&mut state, manager_id);
    }

    /// Number of streams currently admitted for one environment.
    pub fn active_environment_streams(&self, environment_id: Uuid) -> usize {
        self.lock()
            .environment_streams
            .get(&environment_id)
            .copied()
            .unwrap_or(0)
    }

    /// Number of streams currently admitted for one manager.
    pub fn active_manager_streams(&self, manager_id: i64) -> usize {
        self.lock()
            .manager_streams
            .get(&manager_id)
            .copied()
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn has_tunnel(&self, manager_id: i64) -> bool {
        self.lock().tunnels.contains_key(&manager_id)
    }

    #[cfg(test)]
    pub(crate) fn tunnel_lease(&self, manager_id: i64) -> Option<RelayId> {
        self.lock().tunnels.get(&manager_id).map(|slot| slot.lease)
    }
}
