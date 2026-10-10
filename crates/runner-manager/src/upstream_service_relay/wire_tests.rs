//! Whole-wire transport tests (issue 749 P2d, `#[cfg(test)]`).
//!
//! Fake control plane plus loopback agent over real sockets: what these
//! prove is the manager's own multiplexed transport — concurrent streams
//! answered on their own tasks, routed by identifier, cancelled per stream.
//! Every control frame carries its own stream identifier (`df-env-http.v2`).
//! The loopback target is injected through the scoped test executor; Docker
//! admission is covered separately by the fake-CLI suite, not claimed here.

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use runner_protocol::service_relay::{RelayControl, RelayId, ResponseHeadFrame, decode_chunk};

    use super::super::conn;
    use super::super::test_exec::TestExec;
    use super::super::wire_harness::harness::{
        ENV_NAME, TEST_ENV, USERNAME, expected_basic, start_agent,
    };
    use super::super::wire_plane::plane::{decode_text, fake_plane, fresh_id};
    use crate::runtime::execution_environment::service::{
        cancel::ServiceCancelRegistry, credential::ServiceCredentialStore,
    };

    const TIMEOUT: Duration = Duration::from_secs(10);

    /// Live rig: loopback agent, real stored credential, fake control plane,
    /// and the manager tunnel driven between them.
    pub(crate) struct Rig {
        pub token: String,
        pub agent: super::super::wire_harness::harness::Agent,
        pub manager: tokio::task::JoinHandle<()>,
        pub plane: super::super::wire_plane::plane::FakePlane,
        _temp: tempfile::TempDir,
    }

    impl Rig {
        pub(crate) async fn start() -> Self {
            let agent = start_agent().await;
            let temp = tempfile::tempdir().expect("tempdir");
            let store = ServiceCredentialStore::new(temp.path());
            store
                .obtain(TEST_ENV, 4, ENV_NAME)
                .expect("stored credential");
            let token = store
                .read_token(TEST_ENV, 4, ENV_NAME)
                .expect("readable credential");
            let exec = Arc::new(TestExec::new(
                agent.addr,
                USERNAME.to_string(),
                store,
                ENV_NAME.to_string(),
                ServiceCancelRegistry::new(),
            ));
            let (plane_addr, accept) = fake_plane().await;
            let url = format!("ws://{plane_addr}/api/internal/runner-manager/service-relay");
            let (ws, _) = tokio_tungstenite::connect_async(url)
                .await
                .expect("manager connects");
            let manager = tokio::spawn(conn::drive(Arc::clone(&exec), ws));
            let plane = tokio::time::timeout(TIMEOUT, accept)
                .await
                .expect("plane accepts")
                .expect("accept");
            Self {
                token,
                agent,
                manager,
                plane,
                _temp: temp,
            }
        }

        pub(crate) async fn finish(mut self) {
            drop(self.plane);
            tokio::time::timeout(TIMEOUT, &mut self.manager)
                .await
                .expect("drive returns after the socket closes")
                .expect("drive joins");
        }
    }

    /// One decoded inbound frame for the fake plane's event loop.
    #[derive(Debug)]
    pub(crate) enum Event {
        Head(ResponseHeadFrame),
        Data(RelayId, Vec<u8>),
        End(RelayId),
        Error(RelayId, runner_protocol::service_relay::RelayErrorCode),
    }

    pub(crate) async fn next_event(rig: &mut Rig) -> Event {
        let msg = rig.plane.next_frame().await;
        match msg {
            tokio_tungstenite::tungstenite::Message::Text(raw) => match decode_text(&raw) {
                RelayControl::ResponseHead(head) => Event::Head(head),
                RelayControl::End(frame) => Event::End(frame.stream_id),
                RelayControl::Error(frame) => Event::Error(frame.stream_id, frame.code),
                other => panic!("unexpected control frame {other:?}"),
            },
            tokio_tungstenite::tungstenite::Message::Binary(raw) => {
                let chunk = decode_chunk(&raw).expect("decodable data");
                Event::Data(chunk.stream_id, chunk.data.to_vec())
            }
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_binary_echo_round_trip_is_byte_exact() {
        let mut rig = Rig::start().await;
        let stream = fresh_id();
        let body: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        rig.plane
            .send_open(stream, "POST", "v1/echo", body.len() as u64)
            .await;
        rig.plane.send_data(stream, &body).await;
        rig.plane.send_request_end(stream).await;
        let mut received = Vec::new();
        let mut status = 0u16;
        loop {
            match next_event(&mut rig).await {
                Event::Head(head) => {
                    assert_eq!(head.stream_id, stream, "a head answers its own stream");
                    status = head.status;
                    assert_eq!(
                        head.allowed_headers,
                        vec!["content-type: application/json".to_string()]
                    );
                }
                Event::Data(id, bytes) => {
                    assert_eq!(id, stream);
                    received.extend_from_slice(&bytes);
                }
                Event::End(id) => {
                    assert_eq!(id, stream, "an end terminates its own stream");
                    break;
                }
                Event::Error(id, code) => panic!("echo must succeed, got {code:?} for {id}"),
            }
        }
        assert_eq!(status, 200);
        assert_eq!(received, body);
        assert_eq!(
            rig.agent.seen_auth.lock().expect("auth").as_deref(),
            Some(expected_basic(&rig.token).as_str()),
            "the agent must see the real stored credential"
        );
        rig.finish().await;
    }

    #[tokio::test]
    async fn an_sse_stream_stays_open_while_a_binary_request_completes() {
        let mut rig = Rig::start().await;
        let slow = fresh_id();
        let fast = fresh_id();
        rig.plane.send_open(slow, "GET", "v1/sse", 0).await;
        rig.plane.send_request_end(slow).await;
        // The slow stream's head and first chunk arrive before the second
        // stream even opens.
        let mut slow_body = Vec::new();
        let mut slow_head = false;
        while !slow_head || slow_body.is_empty() {
            match next_event(&mut rig).await {
                Event::Head(head) => {
                    assert_eq!(head.stream_id, slow);
                    assert_eq!(
                        head.allowed_headers,
                        vec!["content-type: text/event-stream".to_string()]
                    );
                    slow_head = true;
                }
                Event::Data(id, bytes) => {
                    assert_eq!(id, slow);
                    slow_body.extend_from_slice(&bytes);
                }
                other => panic!("unexpected frame before the fast stream: {other:?}"),
            }
        }
        assert_eq!(slow_body, b"data: one\n\n");
        // The fast request opens, serves fully, and ends while the slow
        // stream is still between its chunks.
        rig.plane.send_open(fast, "GET", "v1/binary", 0).await;
        rig.plane.send_request_end(fast).await;
        let mut fast_body = Vec::new();
        let mut fast_ended = false;
        while !fast_ended {
            match next_event(&mut rig).await {
                Event::Head(head) => {
                    assert_eq!(head.stream_id, fast, "each head answers its own stream");
                    assert_eq!(head.status, 200);
                }
                Event::Data(id, bytes) => {
                    if id == fast {
                        fast_body.extend_from_slice(&bytes);
                    } else {
                        assert_eq!(id, slow);
                        slow_body.extend_from_slice(&bytes);
                    }
                }
                Event::End(id) => {
                    assert_eq!(id, fast, "the fast stream ends first by identifier");
                    fast_ended = true;
                }
                Event::Error(id, code) => {
                    panic!("multiplexed streams must succeed: {code:?} for {id}")
                }
            }
        }
        assert_eq!(fast_body.len(), 100_000);
        let expected: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(fast_body, expected);
        // Now drain the slow stream to its own attributed end.
        let mut tails: HashMap<RelayId, Vec<u8>> = HashMap::new();
        loop {
            match next_event(&mut rig).await {
                Event::Data(id, bytes) => {
                    assert_eq!(id, slow, "only the slow stream remains");
                    tails.entry(id).or_default().extend_from_slice(&bytes);
                }
                Event::End(id) => {
                    assert_eq!(id, slow);
                    break;
                }
                other => panic!("unexpected tail frame: {other:?}"),
            }
        }
        slow_body.extend_from_slice(&tails[&slow]);
        assert_eq!(slow_body, b"data: one\n\ndata: two\n\n");
        rig.finish().await;
    }

    #[tokio::test]
    async fn two_posts_interleave_byte_exact() {
        let mut rig = Rig::start().await;
        let first = fresh_id();
        let second = fresh_id();
        let first_body = b"first-body-bytes".to_vec();
        let second_body = b"second-body-longer-bytes".to_vec();
        // Both opens first, then interleaved body slices, then both ends:
        // parallel bodies must not cross.
        rig.plane
            .send_open(first, "POST", "v1/echo", first_body.len() as u64)
            .await;
        rig.plane
            .send_open(second, "POST", "v1/echo", second_body.len() as u64)
            .await;
        rig.plane.send_data(first, &first_body[..5]).await;
        rig.plane.send_data(second, &second_body[..7]).await;
        rig.plane.send_data(first, &first_body[5..]).await;
        rig.plane.send_data(second, &second_body[7..]).await;
        rig.plane.send_request_end(first).await;
        rig.plane.send_request_end(second).await;
        let mut bodies: HashMap<RelayId, Vec<u8>> = HashMap::new();
        let mut ends = 0;
        while ends < 2 {
            match tokio::time::timeout(TIMEOUT, next_event(&mut rig)).await {
                Ok(Event::Head(head)) => assert_eq!(head.status, 200),
                Ok(Event::Data(id, bytes)) => {
                    bodies.entry(id).or_default().extend_from_slice(&bytes);
                }
                Ok(Event::End(id)) => {
                    assert!(id == first || id == second);
                    ends += 1;
                }
                Ok(Event::Error(id, code)) => panic!("both posts must succeed: {code:?} for {id}"),
                Err(_) => panic!("both interleaved posts must complete"),
            }
        }
        assert_eq!(
            bodies.get(&first).map(Vec::as_slice),
            Some(first_body.as_slice())
        );
        assert_eq!(
            bodies.get(&second).map(Vec::as_slice),
            Some(second_body.as_slice())
        );
        rig.finish().await;
    }
}
