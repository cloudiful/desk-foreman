//! Synchronous open-stream admission decision (issue 749 P2d/P2-1 repair).
//!
//! Extracted from `registry.rs` so the registry never grows past its size
//! budget. This holds only the lock-held checks — live-tunnel presence,
//! identifier reuse (live or recently closed), fixed quotas, and the body
//! bound — and returns the values the registry needs to mint the ticket.
//!
//! The counts returned here are the counts this very admission must reserve
//! against. They are valid only while the caller still holds the lock it
//! took to call this, which is why the registry performs check, ticket mint
//! and route insertion inside one critical section: a snapshot written back
//! after releasing the lock would let two racing opens both observe the same
//! pre-admission count and both be admitted.

use super::{errors::RelayAdmissionError, registry::StreamRequest, registry_state::RegistryState};
use runner_protocol::service_relay::{MAX_BODY_BYTES, admit_stream};

/// Values the registry needs after the checks pass.
///
/// `environment_streams` and `manager_streams` are the counts observed
/// under the lock; the caller adds exactly one to each inside the same
/// critical section, so the reservation is atomic with the check.
pub(super) struct Admission {
    pub(super) tunnel: super::tunnel::TunnelHandle,
    pub(super) lease: runner_protocol::service_relay::RelayId,
    pub(super) epoch: u64,
    pub(super) environment_streams: usize,
    pub(super) manager_streams: usize,
}

/// Check one open against the live tunnel, identifier reuse, quotas and the
/// body bound. Must be called with the registry lock already held, and the
/// caller must complete the reservation before releasing it.
pub(super) fn check_open(
    state: &mut RegistryState,
    request: &StreamRequest,
) -> Result<Admission, RelayAdmissionError> {
    super::registry_state::prune_dead_routes(state);
    let (tunnel, lease) = match state.tunnels.get(&request.manager_id) {
        Some(slot) if !slot.handle.is_closed() => (slot.handle.clone(), slot.lease),
        Some(_) => {
            state.tunnels.remove(&request.manager_id);
            return Err(RelayAdmissionError::NoTunnel);
        }
        None => return Err(RelayAdmissionError::NoTunnel),
    };
    // Identifier reuse on this registry is refused: a recycled id would make
    // two streams unattributable. Live routes and the bounded recently-closed
    // set both count, so delayed frames and immediate reopen races are both
    // covered without an unbounded log.
    if state.routes.contains_key(&request.stream_id) || state.closed.contains(request.stream_id) {
        return Err(RelayAdmissionError::DuplicateStream);
    }
    let environment_streams = state
        .environment_streams
        .get(&request.environment_id)
        .copied()
        .unwrap_or(0);
    let manager_streams = state
        .manager_streams
        .get(&request.manager_id)
        .copied()
        .unwrap_or(0);
    admit_stream(environment_streams, manager_streams)
        .map_err(|_| RelayAdmissionError::StreamLimit)?;
    if request.declared_body_length > MAX_BODY_BYTES {
        return Err(RelayAdmissionError::BodyTooLarge);
    }
    let epoch = state
        .environment_epochs
        .get(&request.environment_id)
        .copied()
        .unwrap_or(0);
    Ok(Admission {
        tunnel,
        lease,
        epoch,
        environment_streams,
        manager_streams,
    })
}
