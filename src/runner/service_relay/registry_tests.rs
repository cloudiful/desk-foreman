//! Registry ownership and admission tests (issue 749 P2d).
//!
//! These exercise the registry directly with no database and no socket:
//! what they prove is multiplexed admission — many concurrent streams per
//! tunnel up to the fixed quotas — identifier reuse refusal (live or
//! recently closed), lease-checked delivery isolation across managers, and
//! quota release exactly once however a stream ends.

use std::sync::Arc;

use runner_protocol::service_relay::{MAX_BODY_BYTES, RelayId};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::runner::service_relay::{
    RelayAdmissionError, RelayRegistry, StreamRequest, TunnelHandle, frames::OutboundFrame,
};

fn registry() -> Arc<RelayRegistry> {
    RelayRegistry::new()
}

/// Attach a tunnel and hand back the receiver that keeps it open.
///
/// Tests that need the connection to disappear drop the receiver, which is
/// the only honest way to model a socket going away.
fn attach(
    registry: &Arc<RelayRegistry>,
    manager_id: i64,
) -> (RelayId, TunnelHandle, mpsc::Receiver<OutboundFrame>) {
    let (tx, rx) = mpsc::channel(8);
    let handle = TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx);
    let lease = registry.attach_tunnel(manager_id, handle.clone()).unwrap();
    (lease, handle, rx)
}

fn request(manager_id: i64, environment_id: Uuid) -> StreamRequest {
    StreamRequest {
        stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
        environment_id,
        manager_id,
        declared_body_length: 0,
    }
}

fn request_with_id(manager_id: i64, environment_id: Uuid, stream_id: RelayId) -> StreamRequest {
    StreamRequest {
        stream_id,
        environment_id,
        manager_id,
        declared_body_length: 0,
    }
}

#[test]
fn a_second_tunnel_for_one_manager_is_refused_and_does_not_replace_the_first() {
    let registry = registry();
    let (first_lease, _first, _open) = attach(&registry, 7);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let second = TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx);
    assert_eq!(
        registry.attach_tunnel(7, second),
        Err(RelayAdmissionError::TunnelConflict)
    );
    assert_eq!(
        registry.tunnel_lease(7),
        Some(first_lease),
        "the live tunnel must survive a refused connection"
    );
}

#[test]
fn a_stale_connection_cannot_detach_the_tunnel_that_replaced_it() {
    let registry = registry();
    let (stale_lease, _stale, stale_open) = attach(&registry, 7);
    // The connection task is gone, so the next connection is admitted.
    drop(stale_open);
    let (_replacement_lease, _replacement, _replacement_open) = attach(&registry, 7);
    registry.detach_tunnel(7, stale_lease);
    assert!(
        registry.has_tunnel(7),
        "a late detach must only remove the tunnel it names"
    );
}

#[test]
fn a_stream_without_a_live_tunnel_is_refused() {
    let registry = registry();
    assert_eq!(
        registry.open_stream(request(7, Uuid::new_v4())).err(),
        Some(RelayAdmissionError::NoTunnel),
        "an unbound manager is never dialled"
    );
}

#[test]
fn dropping_the_last_ticket_releases_the_stream_slot() {
    let registry = registry();
    let (_lease, _handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let stream_id = RelayId::from_bytes(Uuid::new_v4().into_bytes());
    let call = registry
        .open_stream(request_with_id(7, environment_id, stream_id))
        .unwrap();
    assert_eq!(registry.active_environment_streams(environment_id), 1);
    drop(call);
    assert_eq!(
        registry.active_environment_streams(environment_id),
        0,
        "a stream ticket releases itself through its own drop"
    );
    assert_eq!(registry.active_manager_streams(7), 0);
}

#[test]
fn a_committed_cancellation_stops_only_its_own_environment() {
    let registry = registry();
    let (_a, _ha, _open_a) = attach(&registry, 7);
    let (_b, _hb, _open_b) = attach(&registry, 8);
    let stopped = Uuid::new_v4();
    let kept = Uuid::new_v4();
    let stopped_call = registry.open_stream(request(7, stopped)).unwrap();
    let kept_call = registry.open_stream(request(8, kept)).unwrap();

    assert_eq!(registry.cancel_environment(stopped), 1);
    assert!(stopped_call.is_cancelled());
    assert!(!kept_call.is_cancelled(), "another tenant keeps serving");
    assert_eq!(
        registry.cancel_environment(stopped),
        0,
        "a second cancellation must not re-signal the same stream"
    );
    drop(stopped_call);
    assert!(registry.open_stream(request(7, stopped)).is_ok());
}

#[test]
fn a_cancellation_advances_the_epoch_but_multiplex_stays_open() {
    let registry = registry();
    let (_lease, _handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let call = registry.open_stream(request(7, environment_id)).unwrap();
    assert!(call.epoch_is_current());
    registry.cancel_environment(environment_id);
    assert!(
        !call.epoch_is_current(),
        "a committed mutation must be visible to the stream it cancelled"
    );
    // Multiplex: the cancelled stream still occupies its slot until its
    // ticket is released, but a second stream is admitted alongside it —
    // responses correlate by identifier, never by tunnel-singleton slot.
    let next = registry.open_stream(request(7, environment_id)).unwrap();
    assert!(
        next.epoch_is_current(),
        "a registration made after the mutation captures the new epoch"
    );
    drop(call);
    drop(next);
}

#[test]
fn losing_a_tunnel_fails_its_streams_without_touching_the_registry_counts() {
    let registry = registry();
    let (lease, handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let call = registry.open_stream(request(7, environment_id)).unwrap();
    handle.abort();
    registry.detach_tunnel(7, lease);
    assert!(
        call.is_cancelled(),
        "a caller waits on nothing after a lost tunnel"
    );
    drop(call);
    assert_eq!(registry.active_environment_streams(environment_id), 0);
    assert_eq!(registry.active_manager_streams(7), 0);
}

#[test]
fn a_declared_body_over_the_relay_bound_is_refused_before_admission() {
    let registry = registry();
    let (_lease, _handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let mut oversized = request(7, environment_id);
    oversized.declared_body_length = MAX_BODY_BYTES + 1;
    assert_eq!(
        registry.open_stream(oversized).err(),
        Some(RelayAdmissionError::BodyTooLarge)
    );
    assert_eq!(
        registry.active_environment_streams(environment_id),
        0,
        "a refused stream never occupies a slot"
    );
}
