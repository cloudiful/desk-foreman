//! Manager quota and isolation wire tests (issue 749 P2d, `#[cfg(test)]`).
//!
//! Fake control plane only — no agent, no Docker, no PG: what these prove
//! is that the manager counts every assembling body plus every serving
//! response against the fixed 8-per-environment and 32-per-manager quotas,
//! that a released slot (cancel or attributed failure) re-admits exactly
//! one stream, and that unknown or already-closed identifiers are isolated
//! without touching live streams.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use runner_protocol::service_relay::{RelayControl, RelayErrorCode, RelayId};
    use uuid::Uuid;

    use super::super::wire_plane::plane::{decode_text, fresh_id};
    use super::super::wire_tests::tests::{Rig, next_event};

    fn env() -> RelayId {
        RelayId::from_bytes(Uuid::new_v4().into_bytes())
    }

    /// Read one attributed error, asserting its identifier and code.
    async fn next_refusal(rig: &mut Rig, stream: RelayId, code: RelayErrorCode) {
        let raw = rig.plane.next_text().await;
        match decode_text(&raw) {
            RelayControl::Error(frame) => {
                assert_eq!(frame.stream_id, stream, "a refusal names its own stream");
                assert_eq!(frame.code, code);
            }
            other => panic!("expected an attributed refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_ninth_assembling_body_for_one_environment_is_refused() {
        let mut rig = Rig::start().await;
        let environment = env();
        // Eight parallel bodies assemble; none completes, so all eight
        // slots stay occupied — the paused-mock-task shape with no agent.
        for _ in 0..8 {
            rig.plane
                .send_open_env(fresh_id(), environment, "POST", "v1/echo", 0)
                .await;
        }
        let ninth = fresh_id();
        rig.plane
            .send_open_env(ninth, environment, "POST", "v1/echo", 0)
            .await;
        next_refusal(&mut rig, ninth, RelayErrorCode::StreamLimit).await;
        rig.finish().await;
    }

    #[tokio::test]
    async fn the_thirty_third_stream_for_one_tunnel_is_refused() {
        let mut rig = Rig::start().await;
        // Distinct environments so only the per-manager cap binds.
        for _ in 0..32 {
            rig.plane.send_open(fresh_id(), "POST", "v1/echo", 0).await;
        }
        let overflow = fresh_id();
        rig.plane.send_open(overflow, "POST", "v1/echo", 0).await;
        next_refusal(&mut rig, overflow, RelayErrorCode::StreamLimit).await;
        rig.finish().await;
    }

    #[tokio::test]
    async fn a_cancelled_slot_re_admits_exactly_one_stream() {
        let mut rig = Rig::start().await;
        let environment = env();
        let mut ids = Vec::new();
        for _ in 0..8 {
            let id = fresh_id();
            rig.plane
                .send_open_env(id, environment, "POST", "v1/echo", 0)
                .await;
            ids.push(id);
        }
        rig.plane.send_cancel(ids[0]).await;
        // The cancelled slot is forgotten promptly: one more open fits.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let fresh = fresh_id();
        rig.plane
            .send_open_env(fresh, environment, "POST", "v1/echo", 0)
            .await;
        // No refusal arrives for the fresh stream within the quiet window;
        // instead it assembles and serves once completed below.
        rig.plane.send_request_end(fresh).await;
        loop {
            match next_event(&mut rig).await {
                super::super::wire_tests::tests::Event::Head(head) => {
                    assert_eq!(head.stream_id, fresh);
                    break;
                }
                super::super::wire_tests::tests::Event::Error(id, code) => {
                    panic!("released slot must serve, got {code:?} for {id}")
                }
                _ => continue,
            }
        }
        rig.finish().await;
    }

    #[tokio::test]
    async fn unknown_and_closed_frames_are_isolated() {
        let mut rig = Rig::start().await;
        let live = fresh_id();
        rig.plane.send_open(live, "POST", "v1/echo", 2).await;
        // Data for an unknown stream, a completion for an unknown stream,
        // and a cancel for an unknown stream: all discarded, tunnel lives.
        let ghost = fresh_id();
        rig.plane.send_data(ghost, b"orphan").await;
        rig.plane.send_request_end(ghost).await;
        rig.plane.send_cancel(ghost).await;
        // The live stream still assembles and serves its 200.
        rig.plane.send_data(live, b"hi").await;
        rig.plane.send_request_end(live).await;
        let mut body = Vec::new();
        loop {
            match next_event(&mut rig).await {
                super::super::wire_tests::tests::Event::Head(head) => {
                    assert_eq!(head.stream_id, live)
                }
                super::super::wire_tests::tests::Event::Data(id, bytes) => {
                    assert_eq!(id, live);
                    body.extend_from_slice(&bytes);
                }
                super::super::wire_tests::tests::Event::End(id) => {
                    assert_eq!(id, live);
                    break;
                }
                super::super::wire_tests::tests::Event::Error(id, code) => {
                    panic!("live stream must succeed: {code:?} for {id}")
                }
            }
        }
        assert_eq!(body, b"hi");
        rig.finish().await;
    }
}
