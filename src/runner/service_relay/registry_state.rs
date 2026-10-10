//! Registry bookkeeping state (issue 749 P2d).
//!
//! Split from `registry.rs` so no single file exceeds the size budget. This
//! holds only the tunnel/stream tables and the synchronous accounting that
//! keeps them exact; admission, delivery and cancellation stay in the
//! registry itself.
//!
//! Tunnels carry no per-stream position: every frame in both directions
//! carries its own stream identifier (`df-env-http.v2`), so multiplexed
//! streams route by reading the frame rather than by tunnel-singleton slot.

use std::{collections::HashMap, sync::Weak};

use runner_protocol::service_relay::RelayId;
use uuid::Uuid;

use super::{call::CallInner, closed::ClosedStreamIds, tunnel::TunnelHandle};

/// One live manager tunnel.
pub(super) struct TunnelSlot {
    pub(super) lease: RelayId,
    pub(super) handle: TunnelHandle,
}

/// Delivery route for one admitted stream.
///
/// The lease binds the route to the exact connection that opened it: an
/// inbound frame is delivered only when its authenticated manager *and* its
/// lease both match, so a forged cross-manager identifier can never affect
/// another tenant's stream.
pub(super) struct StreamRoute {
    pub(super) manager_id: i64,
    pub(super) lease: RelayId,
    pub(super) environment_id: Uuid,
    pub(super) call: Weak<CallInner>,
}

#[derive(Default)]
pub(super) struct RegistryState {
    pub(super) tunnels: HashMap<i64, TunnelSlot>,
    pub(super) routes: HashMap<RelayId, StreamRoute>,
    pub(super) closed: ClosedStreamIds,
    pub(super) environment_epochs: HashMap<Uuid, u64>,
    pub(super) environment_streams: HashMap<Uuid, usize>,
    pub(super) manager_streams: HashMap<i64, usize>,
}

/// Drop routes whose ticket is already gone, so bookkeeping cannot outlive
/// the stream it describes.
pub(super) fn prune_dead_routes(state: &mut RegistryState) {
    let dead: Vec<RelayId> = state
        .routes
        .iter()
        .filter(|(_, route)| route.call.strong_count() == 0)
        .map(|(stream_id, _)| *stream_id)
        .collect();
    for stream_id in dead {
        if let Some(route) = state.routes.remove(&stream_id) {
            decrement_environment(state, route.environment_id);
            decrement_manager(state, route.manager_id);
        }
    }
}

pub(super) fn decrement_environment(state: &mut RegistryState, environment_id: Uuid) {
    if let Some(value) = state.environment_streams.get_mut(&environment_id) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            state.environment_streams.remove(&environment_id);
        }
    }
}

pub(super) fn decrement_manager(state: &mut RegistryState, manager_id: i64) {
    if let Some(value) = state.manager_streams.get_mut(&manager_id) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            state.manager_streams.remove(&manager_id);
        }
    }
}
