//! Registry multiplex, reuse, lease and quota tests (issue 749 P2d).
//!
//! Split from `registry_tests.rs` so no test file exceeds the size budget.
//! These prove attributed multiplexing directly: concurrent streams share
//! one tunnel by identifier, reuse is refused live and recently closed,
//! delivery isolates by manager and lease, and the 8/32 quotas bind with
//! exact release.

use std::sync::Arc;

use runner_protocol::service_relay::RelayId;
use uuid::Uuid;

use crate::runner::service_relay::{
    RelayAdmissionError, RelayRegistry, StreamDelivery, StreamRequest, frames::InboundEvent,
};

fn registry() -> Arc<RelayRegistry> {
    RelayRegistry::new()
}

fn attach(
    registry: &Arc<RelayRegistry>,
    manager_id: i64,
) -> (
    RelayId,
    crate::runner::service_relay::TunnelHandle,
    tokio::sync::mpsc::Receiver<crate::runner::service_relay::frames::OutboundFrame>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let handle = crate::runner::service_relay::TunnelHandle::new(
        RelayId::from_bytes(Uuid::new_v4().into_bytes()),
        tx,
    );
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
fn a_tunnel_multiplexes_concurrent_streams_by_identifier() {
    let registry = registry();
    let (_lease, _handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let first = registry.open_stream(request(7, environment_id)).unwrap();
    let second = registry.open_stream(request(7, environment_id)).unwrap();
    assert_eq!(registry.active_environment_streams(environment_id), 2);
    assert_eq!(registry.active_manager_streams(7), 2);
    drop(first);
    assert_eq!(registry.active_environment_streams(environment_id), 1);
    drop(second);
    assert_eq!(registry.active_environment_streams(environment_id), 0);
    assert!(
        registry.open_stream(request(7, environment_id)).is_ok(),
        "releasing every stream re-admits the tunnel"
    );
}

#[test]
fn a_reused_identifier_is_refused_live_and_after_close() {
    let registry = registry();
    let (_lease, _handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let stream_id = RelayId::from_bytes(Uuid::new_v4().into_bytes());
    let call = registry
        .open_stream(request_with_id(7, environment_id, stream_id))
        .unwrap();
    assert_eq!(
        registry
            .open_stream(request_with_id(7, environment_id, stream_id))
            .err(),
        Some(RelayAdmissionError::DuplicateStream),
        "a live identifier cannot be reused"
    );
    drop(call);
    assert_eq!(
        registry
            .open_stream(request_with_id(7, environment_id, stream_id))
            .err(),
        Some(RelayAdmissionError::DuplicateStream),
        "a recently closed identifier cannot be immediately reused"
    );
    assert_eq!(
        registry.active_environment_streams(environment_id),
        0,
        "refused reuses never occupy a slot"
    );
}

#[test]
fn delivery_requires_the_stored_manager_and_lease() {
    let registry = registry();
    let (lease_a, _ha, _oa) = attach(&registry, 7);
    let (lease_b, _hb, _ob) = attach(&registry, 8);
    let environment_id = Uuid::new_v4();
    let stream_id = RelayId::from_bytes(Uuid::new_v4().into_bytes());
    let call = registry
        .open_stream(request_with_id(7, environment_id, stream_id))
        .unwrap();
    let response = call.take_response();
    assert!(response.is_some());
    // Forged cross-manager identifier cannot affect the stream.
    assert_eq!(
        registry.deliver(
            8,
            lease_b,
            stream_id,
            InboundEvent::Data(axum::body::Bytes::from_static(b"x"))
        ),
        StreamDelivery::NoRoute
    );
    // Stale lease from the same manager cannot either.
    let stale = RelayId::from_bytes(Uuid::new_v4().into_bytes());
    assert_eq!(
        registry.deliver(
            7,
            stale,
            stream_id,
            InboundEvent::Data(axum::body::Bytes::from_static(b"x"))
        ),
        StreamDelivery::NoRoute
    );
    // The authentic manager and lease do deliver.
    assert_eq!(
        registry.deliver(
            7,
            lease_a,
            stream_id,
            InboundEvent::Data(axum::body::Bytes::from_static(b"x"))
        ),
        StreamDelivery::Delivered
    );
}

#[test]
fn the_ninth_stream_for_one_environment_is_refused() {
    let registry = registry();
    let (_lease, _handle, _open) = attach(&registry, 7);
    let environment_id = Uuid::new_v4();
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(registry.open_stream(request(7, environment_id)).unwrap());
    }
    assert_eq!(registry.active_environment_streams(environment_id), 8);
    assert_eq!(
        registry.open_stream(request(7, environment_id)).err(),
        Some(RelayAdmissionError::StreamLimit),
        "the per-environment cap admits exactly eight"
    );
    drop(held.pop());
    assert!(
        registry.open_stream(request(7, environment_id)).is_ok(),
        "releasing one slot re-admits exactly one stream"
    );
}
