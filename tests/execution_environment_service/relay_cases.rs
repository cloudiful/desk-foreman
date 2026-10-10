//! Relayed request/response cases driven by a mock manager (issue 749 P2d).
//!
//! The mock manager holds a real relay tunnel over the real internal HTTP
//! surface and answers with real relay frames, so these cases exercise the
//! whole control-plane path without a Docker container, an agent process, or
//! an outbound manager client. Every frame carries its own stream identifier
//! (`df-env-http.v2`), so responses multiplex by reading the frame.

use std::time::Duration;

use runner_protocol::service_relay::{
    ErrorFrame, RelayControl, RelayErrorCode, ResponseHeadFrame, StreamFrame, decode_chunk,
    decode_control, encode_chunk, encode_control,
};

use super::relay_helpers::{read_open, relay, respond};
use super::support::ServiceFixture;
use super::ws_client::Frame;

#[tokio::test]
async fn a_relayed_request_carries_the_method_path_query_and_only_permitted_headers() {
    let fixture = ServiceFixture::setup().await;
    let (request, mut tunnel) = relay(
        &fixture,
        "POST",
        "v1/chat/completions?stream=1&tag=a:b",
        &[
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
            (
                "authorization".to_string(),
                "Bearer application-secret".to_string(),
            ),
            ("cookie".to_string(), "session=secret".to_string()),
            ("x-forwarded-for".to_string(), "203.0.113.9".to_string()),
        ],
        b"{}",
    )
    .await;
    let (stream, observed) = read_open(&mut tunnel).await;
    assert_eq!(observed.method, "POST");
    assert_eq!(observed.path, "v1/chat/completions");
    assert_eq!(
        observed.query.as_deref(),
        Some("stream=1&tag=a:b"),
        "the query is opaque text, never a target address"
    );
    assert_eq!(observed.body_length, 2);
    let joined = observed.allowed_headers.join("\n");
    assert!(
        joined.contains("application/json"),
        "content type must cross"
    );
    assert!(joined.contains("text/event-stream"), "accept must cross");
    for forbidden in [
        "application-secret",
        "session=secret",
        "203.0.113.9",
        "Bearer",
    ] {
        assert!(
            !joined.contains(forbidden),
            "{forbidden} must never cross the relay"
        );
    }
    respond(
        &mut tunnel,
        stream,
        200,
        vec!["content-type: application/json".to_string()],
        b"{}",
    )
    .await;
    let outcome = request.await.expect("relay request finishes");
    assert_eq!(outcome.status, 200);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_response_body_is_streamed_and_keeps_only_the_safe_headers() {
    let fixture = ServiceFixture::setup().await;
    let (request, mut tunnel) = relay(&fixture, "GET", "v1/stream", &[], b"").await;
    let (stream, _) = read_open(&mut tunnel).await;
    tunnel
        .send_text(
            &encode_control(&RelayControl::ResponseHead(ResponseHeadFrame {
                stream_id: stream,
                status: 200,
                allowed_headers: vec![
                    "content-type: text/event-stream".to_string(),
                    "set-cookie: session=secret".to_string(),
                    "location: https://elsewhere.invalid".to_string(),
                ],
            }))
            .expect("encode head"),
        )
        .await;
    tunnel
        .send_binary(&encode_chunk(stream, b"data: one\n\n").expect("chunk"))
        .await;
    tunnel
        .send_binary(&encode_chunk(stream, b"data: two\n\n").expect("chunk"))
        .await;
    tunnel
        .send_text(
            &encode_control(&RelayControl::End(StreamFrame { stream_id: stream })).expect("end"),
        )
        .await;
    let outcome = request.await.expect("relay request finishes");
    assert_eq!(outcome.status, 200);
    assert_eq!(
        outcome.header("content-type").as_deref(),
        Some("text/event-stream")
    );
    assert!(
        outcome.header("set-cookie").is_none(),
        "a relayed response must never set a cookie on the caller"
    );
    assert!(outcome.header("location").is_none());
    assert_eq!(outcome.body_text(), "data: one\n\ndata: two\n\n");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_disconnected_tunnel_ends_the_relay_without_a_synthetic_success() {
    let fixture = ServiceFixture::setup().await;
    let (request, mut tunnel) = relay(&fixture, "GET", "v1/models", &[], b"").await;
    let _ = read_open(&mut tunnel).await;
    drop(tunnel);
    let outcome = request.await.expect("relay request finishes");
    assert!(
        outcome.status == 503 || outcome.status == 0,
        "a lost tunnel must not become an empty 200, got {}",
        outcome.status
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_agent_refusal_before_a_response_head_becomes_a_documented_status() {
    let fixture = ServiceFixture::setup().await;
    let (request, mut tunnel) = relay(&fixture, "GET", "v1/missing", &[], b"").await;
    let (stream, _) = read_open(&mut tunnel).await;
    tunnel
        .send_text(
            &encode_control(&RelayControl::Error(ErrorFrame {
                stream_id: stream,
                code: RelayErrorCode::NotFound,
            }))
            .expect("encode refusal"),
        )
        .await;
    let outcome = request.await.expect("relay request finishes");
    assert_eq!(outcome.status, 404);
    assert!(
        !outcome.body_text().contains("df-env-http"),
        "no relay diagnostic may reach the caller"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_body_reaches_the_agent_as_data_frames_and_only_then_as_complete() {
    let fixture = ServiceFixture::setup().await;
    let (request, mut tunnel) = relay(&fixture, "POST", "v1/chat", &[], b"{\"short\":true}").await;
    let (stream, observed) = read_open(&mut tunnel).await;
    assert_eq!(
        observed.body_length, 14,
        "the declared length is what the agent is told to expect"
    );
    let body = b"{\"short\":true}";
    let mut forwarded = Vec::new();
    let mut completed = false;
    // The body may be split into several data frames; completion may only be
    // reported once every declared byte has arrived, which is exactly what
    // makes a truncated client body cancel instead of complete.
    while !completed {
        match tunnel
            .next_frame(Duration::from_secs(10))
            .await
            .expect("a body or completion frame")
        {
            Frame::Binary(bytes) => {
                let chunk = decode_chunk(&bytes).expect("a decodable data frame");
                assert_eq!(
                    chunk.stream_id, stream,
                    "a body frame must belong to the stream it was opened for"
                );
                forwarded.extend_from_slice(chunk.data);
            }
            Frame::Text(text) => match decode_control(&text).expect("decodable") {
                RelayControl::RequestEnd(frame) => {
                    assert_eq!(frame.stream_id, stream, "completion carries its own stream");
                    completed = true;
                }
                _ => continue,
            },
            other => panic!("unexpected frame {other:?}"),
        }
    }
    assert_eq!(
        forwarded, body,
        "the forwarded bytes are the client's bytes"
    );
    let _ = request.await;
    fixture.teardown().await;
}
