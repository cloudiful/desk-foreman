//! Relay concurrency, cancellation and bound cases (issue 749 P2d).
//!
//! Split from `relay_cases.rs` so no test file exceeds the size budget. The
//! mock manager and the shared helpers in `relay_helpers.rs` are unchanged;
//! these cases cover multiplexed races and refusals: concurrent streams share
//! one tunnel by identifier, orphan and cross-stream frames are isolated,
//! committed versus refused invalidation, and oversized bodies.

use std::time::Duration;

use runner_protocol::service_relay::{RelayId, encode_chunk};
use uuid::Uuid;

use super::relay_helpers::{PROTOCOL, fence_header, read_open, relay, respond};
use super::support::{ServiceFixture, call, split_base};
use super::ws_client::WsClient;

#[tokio::test]
async fn concurrent_requests_share_one_tunnel_by_identifier() {
    let fixture = ServiceFixture::setup().await;
    let (first_request, mut tunnel) = relay(&fixture, "GET", "v1/first", &[], b"").await;
    let (first_stream, _) = read_open(&mut tunnel).await;
    // A second request on the same tunnel is admitted alongside the first:
    // responses correlate by stream identifier, never by tunnel position.
    let base = fixture.base.clone();
    let path = fixture.service_path("v1/second");
    let token = fixture.application_token.clone();
    let second_request = tokio::spawn(async move {
        call(&base, "GET", &path, Some(&token), &[fence_header(4)], b"").await
    });
    let (second_stream, _) = read_open(&mut tunnel).await;
    assert_ne!(first_stream, second_stream);
    respond(&mut tunnel, second_stream, 200, vec![], b"second").await;
    respond(&mut tunnel, first_stream, 200, vec![], b"first").await;
    let first_outcome = first_request.await.expect("first finishes");
    let second_outcome = second_request.await.expect("second finishes");
    assert_eq!(first_outcome.status, 200);
    assert_eq!(first_outcome.body_text(), "first");
    assert_eq!(second_outcome.status, 200);
    assert_eq!(second_outcome.body_text(), "second");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_data_frame_for_an_unknown_stream_is_discarded_rather_than_buffered() {
    let fixture = ServiceFixture::setup().await;
    let (request, mut tunnel) = relay(&fixture, "GET", "v1/first", &[], b"").await;
    let (stream, _) = read_open(&mut tunnel).await;
    // An orphan frame for a stream that does not exist neither blocks nor
    // poisons the tunnel, and is not buffered for a later reader.
    tunnel
        .send_binary(
            &encode_chunk(RelayId::from_bytes(Uuid::new_v4().into_bytes()), b"orphan")
                .expect("chunk"),
        )
        .await;
    respond(&mut tunnel, stream, 204, vec![], b"").await;
    let outcome = request.await.expect("relay request finishes");
    assert_eq!(outcome.status, 204);
    drop(tunnel);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_committed_invalidation_cancels_a_live_stream_and_a_refused_one_does_not() {
    let fixture = ServiceFixture::setup().await;
    let mut tunnel = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("mock manager tunnel");
    let start = |suffix: &'static str| {
        let base = fixture.base.clone();
        let path = fixture.service_path(suffix);
        let token = fixture.application_token.clone();
        tokio::spawn(async move {
            call(&base, "GET", &path, Some(&token), &[fence_header(4)], b"").await
        })
    };
    let invalidate = |expected: i64| {
        let base = fixture.base.clone();
        let path = format!(
            "/api/internal/execution-environments/{}/invalidate",
            fixture.environment_id
        );
        let token = fixture.application_token.clone();
        let json = ("content-type".to_string(), "application/json".to_string());
        let body = format!("{{\"expected_generation\":{expected}}}");
        async move {
            call(
                &base,
                "POST",
                &path,
                Some(&token),
                std::slice::from_ref(&json),
                body.as_bytes(),
            )
            .await
        }
    };

    // A stale invalidate commits nothing, so the stream must keep running.
    let kept = start("v1/kept");
    let (kept_stream, _) = read_open(&mut tunnel).await;
    assert_eq!(
        invalidate(99).await.status,
        409,
        "a stale invalidate commits nothing"
    );
    respond(&mut tunnel, kept_stream, 200, vec![], b"kept").await;
    let outcome = kept
        .await
        .expect("the refused mutation leaves the stream alone");
    assert_eq!(outcome.status, 200);
    assert_eq!(outcome.body_text(), "kept");

    // A committed invalidate does stop the next stream on that tunnel.
    let cancelled = start("v1/cancelled");
    let _ = read_open(&mut tunnel).await;
    assert_eq!(
        invalidate(4).await.status,
        200,
        "the committed mutation must succeed"
    );
    let outcome = cancelled
        .await
        .expect("the relay settles after cancellation");
    assert_ne!(
        outcome.status, 200,
        "a cancelled stream must not be reported as a clean success"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_oversized_declared_body_is_refused_before_a_stream_is_opened() {
    let fixture = ServiceFixture::setup().await;
    // The declared length is over the relay bound and the body really is that
    // long, so the request is well formed: whatever refuses it, it must be
    // the gateway answering 413 rather than an HTTP layer rejecting framing.
    let over = 2 * 1024 * 1024 + 1;
    let status = raw_service_call(&fixture, &over.to_string(), over).await;
    assert_eq!(
        status, 413,
        "a well-formed length over the relay bound is the published size \
         refusal, not a malformed request"
    );
    fixture.teardown().await;
}

/// Independent reviewer regression probe (issue 749 final round 3).
///
/// A `Content-Length` that is present but unparseable is a malformed
/// request, and must stay 400: folding it into the 413 size refusal would
/// tell a caller its body was too large when the header itself was wrong.
#[tokio::test]
async fn a_malformed_content_length_stays_a_bad_request() {
    let fixture = ServiceFixture::setup().await;
    for value in ["not-a-number", "", "-1"] {
        let status = raw_service_call(&fixture, value, 0).await;
        assert_eq!(status, 400, "content-length {value:?} must stay a 400");
    }
    fixture.teardown().await;
}

/// Send one raw service request with a verbatim `Content-Length`.
///
/// The shared helper rewrites `Content-Length` from the real body, which
/// would make an over-bound case unreachable and would hide the gateway's own
/// answer behind an HTTP-layer refusal. Writing the body is best-effort: the
/// gateway answers from the headers alone and closes, so a reset while
/// sending is the refusal, not a test failure.
async fn raw_service_call(fixture: &ServiceFixture, declared: &str, body_len: usize) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let base = fixture.base.clone();
    let path = fixture.service_path("v1/chat");
    let token = fixture.application_token.clone();
    let (host, port) = split_base(&base);
    let mut stream = tokio::net::TcpStream::connect(format!("{host}:{port}"))
        .await
        .expect("connect");
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\
         Authorization: Bearer {token}\r\nX-DF-External-User-Id: integration-test-user\r\n\
         Content-Type: application/json\r\nX-Execution-Environment-Generation: 4\r\n\
         Content-Length: {declared}\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return 0;
    }
    let chunk = vec![b'x'; 64 * 1024];
    let mut sent = 0usize;
    while sent < body_len {
        let take = chunk.len().min(body_len - sent);
        if stream.write_all(&chunk[..take]).await.is_err() {
            break;
        }
        sent += take;
    }
    let mut raw = Vec::new();
    if tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw))
        .await
        .is_err()
    {
        return 0;
    }
    String::from_utf8_lossy(&raw)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}
