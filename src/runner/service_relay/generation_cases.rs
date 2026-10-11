use std::sync::{Arc, Barrier};

use runner_protocol::service_relay::RelayId;
use uuid::Uuid;

use crate::runner::service_relay::{
    RelayAdmissionError, RelayRegistry, StreamRequest, TunnelHandle,
};

fn attach(
    registry: &Arc<RelayRegistry>,
) -> tokio::sync::mpsc::Receiver<super::frames::OutboundFrame> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let tunnel = TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx);
    registry.attach_tunnel(7, tunnel).unwrap();
    rx
}

fn request(environment_id: Uuid, generation: i64) -> StreamRequest {
    StreamRequest {
        stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
        environment_id,
        generation,
        manager_id: 7,
        declared_body_length: 0,
    }
}

#[test]
fn delayed_older_cancellation_preserves_successor_streams_and_fences_old_generation() {
    let registry = RelayRegistry::new();
    let _open = attach(&registry);
    let environment_id = Uuid::new_v4();
    let old = registry.open_stream(request(environment_id, 4)).unwrap();
    let successor_before_callback = registry.open_stream(request(environment_id, 5)).unwrap();

    assert_eq!(registry.cancel_environment_through(environment_id, 4), 1);
    assert!(old.is_cancelled());
    assert!(!successor_before_callback.is_cancelled());
    assert!(successor_before_callback.epoch_is_current());
    assert_eq!(
        registry.open_stream(request(environment_id, 4)).err(),
        Some(RelayAdmissionError::EpochChanged)
    );

    let successor_after_callback = registry.open_stream(request(environment_id, 5)).unwrap();
    assert!(successor_after_callback.epoch_is_current());
    assert!(!successor_after_callback.is_cancelled());
    assert_eq!(registry.cancel_environment_through(environment_id, 3), 0);
    assert!(successor_before_callback.epoch_is_current());
    assert!(successor_after_callback.epoch_is_current());

    assert_eq!(registry.cancel_environment_through(environment_id, 5), 2);
    let newest = registry.open_stream(request(environment_id, 6)).unwrap();
    assert!(newest.epoch_is_current());
    assert!(!newest.is_cancelled());
}

#[test]
fn successor_admission_racing_an_older_cancel_is_never_invalidated() {
    for _ in 0..64 {
        let registry = RelayRegistry::new();
        let _open = attach(&registry);
        let environment_id = Uuid::new_v4();
        let barrier = Arc::new(Barrier::new(3));
        let opening_registry = Arc::clone(&registry);
        let opening_barrier = Arc::clone(&barrier);
        let opening = std::thread::spawn(move || {
            opening_barrier.wait();
            opening_registry.open_stream(request(environment_id, 5))
        });
        let cancelling_registry = Arc::clone(&registry);
        let cancelling_barrier = Arc::clone(&barrier);
        let cancelling = std::thread::spawn(move || {
            cancelling_barrier.wait();
            cancelling_registry.cancel_environment_through(environment_id, 4)
        });
        barrier.wait();

        let successor = opening.join().expect("open thread").unwrap();
        assert_eq!(cancelling.join().expect("cancel thread"), 0);
        assert!(!successor.is_cancelled());
        assert!(successor.epoch_is_current());
    }
}
