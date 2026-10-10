//! Cancellation, teardown and refusal wire tests (issue 749 P2d,
//! `#[cfg(test)]`).
//!
//! Shares the [`super::wire_tests`] rig: fake control plane plus loopback
//! agent with a real stored credential. Every frame carries its own stream
//! identifier, so cancellation targets exactly one stream and delayed frames
//! for closed streams are isolated without touching live ones.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use runner_protocol::service_relay::{RelayControl, RelayErrorCode};

    use super::super::wire_plane::plane::{decode_text, fresh_id};
    use super::super::wire_tests::tests::{Event, Rig, next_event};

    /// Quiet window past the agent's 500ms second-chunk delay: a live
    /// stream would speak inside it, a cancelled one never does.
    const QUIET: Duration = Duration::from_millis(800);

    /// Read one event, or `None` when the peer stays quiet.
    async fn quiet_event(rig: &mut Rig) -> Option<Event> {
        tokio::time::timeout(QUIET, next_event(rig)).await.ok()
    }

    /// The manager drive must end after a violation or disconnect.
    async fn drive_must_end(rig: &mut Rig) {
        tokio::time::timeout(Duration::from_secs(10), &mut rig.manager)
            .await
            .expect("drive must end after the violation")
            .expect("drive joins");
    }

    #[tokio::test]
    async fn a_cancel_kills_only_its_stream() {
        let mut rig = Rig::start().await;
        let slow = fresh_id();
        rig.plane.send_open(slow, "GET", "v1/sse", 0).await;
        rig.plane.send_request_end(slow).await;
        // Head plus first chunk prove the stream is serving.
        let mut got_data = false;
        for _ in 0..4 {
            match next_event(&mut rig).await {
                Event::Head(_) => {}
                Event::Data(id, _) => {
                    assert_eq!(id, slow);
                    got_data = true;
                    break;
                }
                other => panic!("unexpected frame before cancel: {other:?}"),
            }
        }
        assert!(got_data);
        rig.plane.send_cancel(slow).await;
        // The cancelled stream sends nothing more, yet the tunnel lives:
        // a fresh stream serves a full 200 afterwards.
        assert!(
            quiet_event(&mut rig).await.is_none(),
            "a cancelled stream must go quiet"
        );
        let fast = fresh_id();
        rig.plane.send_open(fast, "POST", "v1/echo", 2).await;
        rig.plane.send_data(fast, b"hi").await;
        rig.plane.send_request_end(fast).await;
        let mut body = Vec::new();
        let mut status = 0u16;
        loop {
            match next_event(&mut rig).await {
                Event::Head(head) => {
                    assert_eq!(head.stream_id, fast);
                    status = head.status;
                }
                Event::Data(id, bytes) => {
                    assert_eq!(id, fast);
                    body.extend_from_slice(&bytes);
                }
                Event::End(id) => {
                    assert_eq!(id, fast);
                    break;
                }
                Event::Error(id, code) => panic!("fresh stream must succeed: {code:?} for {id}"),
            }
        }
        assert_eq!(status, 200);
        assert_eq!(body, b"hi");
        rig.finish().await;
    }

    #[tokio::test]
    async fn a_socket_disconnect_cleans_every_stream() {
        let mut rig = Rig::start().await;
        let slow = fresh_id();
        rig.plane.send_open(slow, "GET", "v1/sse", 0).await;
        rig.plane.send_request_end(slow).await;
        match next_event(&mut rig).await {
            Event::Head(_) => {}
            other => panic!("expected the SSE head, got {other:?}"),
        }
        // Tear the socket down mid-serve: the drive returns and the agent
        // observes the dropped reply.
        let Rig {
            manager,
            plane,
            agent,
            ..
        } = rig;
        drop(plane);
        let mut manager = manager;
        tokio::time::timeout(Duration::from_secs(10), &mut manager)
            .await
            .expect("drive returns after disconnect")
            .expect("drive joins");
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(
            agent.dropped.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the abandoned agent reply must observe the disconnect"
        );
    }

    #[tokio::test]
    async fn a_reused_identifier_is_refused_with_an_attributed_error() {
        let mut rig = Rig::start().await;
        let stream = fresh_id();
        rig.plane.send_open(stream, "GET", "v1/sse", 0).await;
        // Same identifier while still assembling: attributed refusal, tunnel
        // stays up so unrelated streams are never killed by one reuse.
        rig.plane.send_open(stream, "GET", "v1/sse", 0).await;
        match rig.plane.next_text().await {
            raw => match decode_text(&raw) {
                RelayControl::Error(frame) => {
                    assert_eq!(frame.stream_id, stream);
                    assert_eq!(frame.code, RelayErrorCode::Conflict);
                }
                other => panic!("a reused identifier must be refused, got {other:?}"),
            },
        }
        // The first stream still assembles and serves its 200.
        rig.plane.send_request_end(stream).await;
        loop {
            match next_event(&mut rig).await {
                Event::Head(head) => assert_eq!(head.stream_id, stream),
                Event::Data(id, _) => assert_eq!(id, stream),
                Event::End(id) => {
                    assert_eq!(id, stream);
                    break;
                }
                Event::Error(id, code) => panic!("first stream must succeed: {code:?} for {id}"),
            }
        }
        rig.finish().await;
    }

    #[tokio::test]
    async fn an_oversized_body_fails_only_its_stream() {
        let mut rig = Rig::start().await;
        let stream = fresh_id();
        rig.plane.send_open(stream, "POST", "v1/echo", 1).await;
        rig.plane.send_data(stream, b"ab").await;
        // Per-stream failure carries the offending identifier; the tunnel
        // stays up for unrelated streams.
        match tokio::time::timeout(Duration::from_secs(10), next_event(&mut rig)).await {
            Ok(Event::Error(id, _)) => assert_eq!(id, stream),
            Ok(other) => panic!("an oversized body must fail its stream, got {other:?}"),
            Err(_) => panic!("an oversized body must fail fast with an attributed error"),
        }
        // A fresh stream still serves afterwards.
        let fresh = fresh_id();
        rig.plane.send_open(fresh, "POST", "v1/echo", 2).await;
        rig.plane.send_data(fresh, b"hi").await;
        rig.plane.send_request_end(fresh).await;
        let mut body = Vec::new();
        loop {
            match next_event(&mut rig).await {
                Event::Head(head) => assert_eq!(head.stream_id, fresh),
                Event::Data(id, bytes) => {
                    assert_eq!(id, fresh);
                    body.extend_from_slice(&bytes);
                }
                Event::End(id) => {
                    assert_eq!(id, fresh);
                    break;
                }
                Event::Error(id, code) => panic!("fresh stream must succeed: {code:?} for {id}"),
            }
        }
        assert_eq!(body, b"hi");
        rig.finish().await;
    }

    #[tokio::test]
    async fn parallel_bodies_assemble_independently() {
        let mut rig = Rig::start().await;
        let first = fresh_id();
        let second = fresh_id();
        // No RequestEnd yet for either: both assemble in parallel.
        rig.plane.send_open(first, "POST", "v1/echo", 2).await;
        rig.plane.send_open(second, "POST", "v1/echo", 2).await;
        rig.plane.send_data(first, b"hi").await;
        rig.plane.send_data(second, b"yo").await;
        rig.plane.send_request_end(first).await;
        rig.plane.send_request_end(second).await;
        let mut bodies = std::collections::HashMap::new();
        let mut ends = 0;
        while ends < 2 {
            match next_event(&mut rig).await {
                Event::Head(_) => {}
                Event::Data(id, bytes) => {
                    bodies
                        .entry(id)
                        .or_insert_with(Vec::new)
                        .extend_from_slice(&bytes);
                }
                Event::End(_) => ends += 1,
                Event::Error(id, code) => panic!("both must succeed: {code:?} for {id}"),
            }
        }
        assert_eq!(
            bodies.get(&first).map(Vec::as_slice),
            Some(b"hi".as_slice())
        );
        assert_eq!(
            bodies.get(&second).map(Vec::as_slice),
            Some(b"yo".as_slice())
        );
        rig.finish().await;
    }
}
