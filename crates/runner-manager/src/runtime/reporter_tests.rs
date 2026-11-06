use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, RwLock, oneshot},
    time::timeout,
};

use runner_protocol::{RunnerLifecycleEvent, RunnerLifecycleStatus, RunnerOwner};

use super::{
    Envelope, RunnerLifecycleReporter, enqueue_due_heartbeats, fail_acked_pending, queue_event,
};
use crate::config::SharedRunnerManagerConfig;

fn event_for(user_id: i64, status: RunnerLifecycleStatus) -> RunnerLifecycleEvent {
    RunnerLifecycleEvent {
        owner: RunnerOwner::InternalUser { user_id },
        container_name: format!("desk-foreman-runner-user-{user_id}"),
        container_id: None,
        status,
        workspace_root: None,
        runtime: None,
        runtime_class: None,
        image_name: None,
        network_enabled: None,
        last_error: None,
    }
}

fn event(status: RunnerLifecycleStatus) -> RunnerLifecycleEvent {
    event_for(1, status)
}

fn envelope(status: RunnerLifecycleStatus) -> Envelope {
    Envelope {
        event: event(status),
        ack: None,
    }
}

fn config_for(addr: std::net::SocketAddr) -> SharedRunnerManagerConfig {
    let mut config = super::super::docker_tests::test_config();
    config.control_plane_url = Some(format!("http://{addr}"));
    config.auth_token = "test-token".to_string();
    Arc::new(RwLock::new(config))
}

async fn read_request(socket: &mut TcpStream) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(headers_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let content_length = String::from_utf8_lossy(&buffer[..headers_end])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buffer.len() >= headers_end + 4 + content_length {
                return;
            }
        }
    }
}

async fn respond(socket: &mut TcpStream, status: &str) {
    socket
        .write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
}

#[test]
fn removal_event_is_not_overwritten_by_later_activity() {
    let mut pending = HashMap::new();
    queue_event(&mut pending, envelope(RunnerLifecycleStatus::Removed));
    queue_event(&mut pending, envelope(RunnerLifecycleStatus::Running));
    assert_eq!(
        pending
            .get("user:1")
            .expect("event should remain queued")
            .event
            .status,
        RunnerLifecycleStatus::Removed
    );
}

#[test]
fn queued_event_is_retained_when_newer_event_replaces_it() {
    let mut pending = HashMap::new();
    queue_event(&mut pending, envelope(RunnerLifecycleStatus::Running));
    let removed = event(RunnerLifecycleStatus::Removed);
    queue_event(
        &mut pending,
        Envelope {
            event: removed.clone(),
            ack: None,
        },
    );
    assert_eq!(
        pending.get("user:1").expect("event should exist").event,
        removed
    );
}

#[test]
fn superseded_pending_ack_completes_with_a_fixed_error() {
    let mut pending = HashMap::new();
    let (first_ack, mut first) = oneshot::channel();
    queue_event(
        &mut pending,
        Envelope {
            event: event(RunnerLifecycleStatus::Running),
            ack: Some(first_ack),
        },
    );
    let (second_ack, mut second) = oneshot::channel();
    queue_event(
        &mut pending,
        Envelope {
            event: event(RunnerLifecycleStatus::Running),
            ack: Some(second_ack),
        },
    );

    let first = first
        .try_recv()
        .expect("the replaced acknowledgement must complete");
    assert!(first.is_err());
    assert!(first.unwrap_err().to_string().contains("superseded"));
    assert!(matches!(
        second.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
}

#[test]
fn a_pending_terminal_event_rejects_a_new_acked_running_report() {
    let mut pending = HashMap::new();
    queue_event(&mut pending, envelope(RunnerLifecycleStatus::Removed));
    let (ack, mut receipt) = oneshot::channel();
    queue_event(
        &mut pending,
        Envelope {
            event: event(RunnerLifecycleStatus::Running),
            ack: Some(ack),
        },
    );

    let result = receipt
        .try_recv()
        .expect("the rejected acknowledgement must complete");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("superseded"));
}

#[test]
fn automatic_heartbeat_does_not_supersede_a_pending_ack() {
    let mut pending = HashMap::new();
    let (ack, mut receipt) = oneshot::channel();
    let running = event(RunnerLifecycleStatus::Running);
    queue_event(
        &mut pending,
        Envelope {
            event: running.clone(),
            ack: Some(ack),
        },
    );
    let mut known = HashMap::new();
    known.insert(running.owner.stable_key(), running);
    enqueue_due_heartbeats(&mut pending, &known, &HashMap::new(), Instant::now());

    assert!(matches!(
        receipt.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(pending.len(), 1);
    assert!(pending.values().next().unwrap().ack.is_some());
}

#[test]
fn failed_batch_fails_acked_reports_and_preserves_unacked_ones() {
    let mut pending = HashMap::new();
    let (ack, mut receipt) = oneshot::channel();
    queue_event(
        &mut pending,
        Envelope {
            event: event_for(1, RunnerLifecycleStatus::Running),
            ack: Some(ack),
        },
    );
    queue_event(
        &mut pending,
        Envelope {
            event: event_for(2, RunnerLifecycleStatus::Removed),
            ack: None,
        },
    );
    fail_acked_pending(&mut pending);

    let failed = receipt
        .try_recv()
        .expect("the failed acknowledgement must complete");
    assert!(failed.is_err());
    assert!(failed.unwrap_err().to_string().contains("failed before"));
    assert_eq!(pending.len(), 1);
    assert!(pending.contains_key("user:2"));
}

#[tokio::test]
async fn noop_acknowledges_immediately() {
    let reporter = RunnerLifecycleReporter::noop();
    let started = Instant::now();
    reporter
        .report_and_wait(event(RunnerLifecycleStatus::Running))
        .await
        .expect("the test noop must acknowledge");
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[tokio::test]
async fn first_running_ack_succeeds_only_after_http_2xx() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen, seen_rx) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let server_release = release.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let _ = seen.send(());
        server_release.notified().await;
        respond(&mut socket, "204 No Content").await;
    });
    let reporter = RunnerLifecycleReporter::spawn(config_for(addr));
    let reporter_for_wait = reporter.clone();
    let mut waiter = tokio::spawn(async move {
        reporter_for_wait
            .report_and_wait(event(RunnerLifecycleStatus::Running))
            .await
    });

    seen_rx
        .await
        .expect("the report must reach the control plane");
    assert!(
        timeout(Duration::from_millis(150), &mut waiter)
            .await
            .is_err(),
        "the acknowledgement must not complete before the HTTP response"
    );
    release.notify_one();
    timeout(Duration::from_secs(2), waiter)
        .await
        .expect("a 2xx must complete the acknowledgement")
        .expect("the reporter task must finish")
        .expect("a 2xx must be a success");
    server.await.unwrap();
}

#[tokio::test]
async fn non_success_and_disconnect_fail_with_a_fixed_error() {
    for mode in ["500", "disconnect"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            if mode == "500" {
                read_request(&mut socket).await;
                respond(&mut socket, "500 Internal Server Error").await;
            }
        });
        let reporter = RunnerLifecycleReporter::spawn(config_for(addr));
        let error = timeout(
            Duration::from_secs(2),
            reporter.report_and_wait(event(RunnerLifecycleStatus::Running)),
        )
        .await
        .expect("a failed report must remain bounded")
        .expect_err("a non-2xx or disconnect must fail");
        let message = error.to_string();
        assert!(
            message.contains("runner lifecycle report failed before acknowledgement"),
            "{mode}: {message}"
        );
        assert!(!message.contains(&addr.to_string()));
        assert!(!message.contains("test-token"));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn missing_control_plane_url_fails_an_acked_report() {
    let mut config = super::super::docker_tests::test_config();
    config.control_plane_url = None;
    let reporter = RunnerLifecycleReporter::spawn(Arc::new(RwLock::new(config)));
    let error = timeout(
        Duration::from_secs(2),
        reporter.report_and_wait(event(RunnerLifecycleStatus::Running)),
    )
    .await
    .expect("a missing control-plane URL must fail closed")
    .expect_err("a missing control-plane URL must not acknowledge");
    assert!(
        error
            .to_string()
            .contains("runner lifecycle report failed before acknowledgement")
    );
}
