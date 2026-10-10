//! Concurrent-admission burst probes (issue 749 final round 3 repair,
//! `#[cfg(test)]`).
//!
//! Split from `registry_multiplex_tests.rs` so no test file exceeds the size
//! budget. Both probes race a synchronized burst at the fixed caps and
//! assert that exactly one racer wins — the regression that a non-atomic
//! check-then-reserve admission used to let several racers through.
//!
//! Winners are held alive until the burst joins, exactly as a gateway holds
//! a call ticket for the whole request: a winner that is dropped immediately
//! releases its slot again, so measuring the cap against tickets that no
//! longer exist would prove nothing about it.

use std::sync::Arc;

use runner_protocol::service_relay::RelayId;
use uuid::Uuid;

use crate::runner::service_relay::{RelayRegistry, StreamRequest};

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

/// Race `racers` threads at `held` occupied environment slots and assert the
/// cap admits exactly one of them, with the slot still held afterwards.
fn burst_at_environment_cap(held: usize, racers: usize, rounds: usize) {
    for _round in 0..rounds {
        let registry = registry();
        let (_lease, _handle, _open) = attach(&registry, 7);
        let environment_id = Uuid::new_v4();
        let mut held_streams = Vec::new();
        for _ in 0..held {
            held_streams.push(registry.open_stream(request(7, environment_id)).unwrap());
        }
        assert_eq!(registry.active_environment_streams(environment_id), held);

        let barrier = Arc::new(std::sync::Barrier::new(racers));
        let admitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let winners: Arc<std::sync::Mutex<Vec<crate::runner::service_relay::RelayCall>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for _ in 0..racers {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            let admitted = Arc::clone(&admitted);
            let winners = Arc::clone(&winners);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                if let Ok(call) = registry.open_stream(request(7, environment_id)) {
                    admitted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    winners.lock().expect("winner slot").push(call);
                }
            }));
        }
        for thread in threads {
            thread.join().expect("racer joins");
        }
        assert_eq!(
            admitted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the environment cap must admit exactly one racer (observed {})",
            admitted.load(std::sync::atomic::Ordering::SeqCst)
        );
        assert_eq!(
            registry.active_environment_streams(environment_id),
            held + 1,
            "the held streams plus the single winner are exactly the cap"
        );
        drop(std::mem::take(&mut *winners.lock().expect("winner slot")));
        assert_eq!(registry.active_environment_streams(environment_id), held);
    }
}

/// Race `racers` threads at `held` occupied manager slots, each naming its
/// own environment so only the per-manager cap binds.
fn burst_at_manager_cap(held: usize, racers: usize, rounds: usize) {
    for _round in 0..rounds {
        let registry = registry();
        let (_lease, _handle, _open) = attach(&registry, 7);
        let mut held_streams = Vec::new();
        for _ in 0..held {
            held_streams.push(registry.open_stream(request(7, Uuid::new_v4())).unwrap());
        }
        assert_eq!(registry.active_manager_streams(7), held);

        let barrier = Arc::new(std::sync::Barrier::new(racers));
        let admitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let winners: Arc<std::sync::Mutex<Vec<crate::runner::service_relay::RelayCall>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for _ in 0..racers {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            let admitted = Arc::clone(&admitted);
            let winners = Arc::clone(&winners);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                if let Ok(call) = registry.open_stream(request(7, Uuid::new_v4())) {
                    admitted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    winners.lock().expect("winner slot").push(call);
                }
            }));
        }
        for thread in threads {
            thread.join().expect("racer joins");
        }
        assert_eq!(
            admitted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the manager cap must admit exactly one racer (observed {})",
            admitted.load(std::sync::atomic::Ordering::SeqCst)
        );
        assert_eq!(registry.active_manager_streams(7), held + 1);
        drop(std::mem::take(&mut *winners.lock().expect("winner slot")));
        assert_eq!(
            registry.active_manager_streams(7),
            held,
            "releasing the winner frees exactly one manager slot"
        );
    }
}

#[test]
fn a_concurrent_burst_at_the_environment_cap_admits_exactly_one() {
    burst_at_environment_cap(7, 12, 40);
}

#[test]
fn a_concurrent_burst_at_the_manager_cap_admits_exactly_one() {
    burst_at_manager_cap(31, 12, 25);
}
