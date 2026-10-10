//! WebSocket transport-bound probes (issue 749 final round 3 repair,
//! `#[cfg(test)]`).
//!
//! Split from `admission_cases.rs` so no test file exceeds the size budget.
//! The tunnel ingress bounds both message and frame size at the upgrade, so
//! an oversized peer is refused by the codec before the relay decoder runs,
//! and the refusal is scoped to that peer rather than poisoning the manager.

use std::time::Duration;

use super::relay_helpers::{fence_header, read_open, respond};
use super::support::{ServiceFixture, call};
use super::ws_client::{Frame, WsClient};

const PROTOCOL: &str = "df-env-http.v2";

#[tokio::test]
async fn an_oversized_transport_message_is_refused_at_the_upgrade_bounds() {
    let fixture = ServiceFixture::setup().await;
    let mut tunnel = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("mock manager tunnel");
    // Well past the published 64 KiB transport bound. The server config
    // bounds both message and frame size at the upgrade, so this is refused
    // by the codec before the relay's own decoder ever sees it.
    let oversized = "x".repeat(256 * 1024);
    let write_reset = tunnel.try_send_text(&oversized).await.is_err();
    // Either the peer reset us mid-write or it answers with a close; both
    // are the bounded refusal, and neither may produce relay traffic.
    let closed = write_reset
        || matches!(
            tunnel.next_frame(Duration::from_secs(10)).await,
            Err(_) | Ok(Frame::Close)
        );
    assert!(closed, "an oversized message must end the tunnel");
    drop(tunnel);

    let mut replacement = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("a replacement tunnel");
    let request = tokio::spawn({
        let base = fixture.base.clone();
        let path = fixture.service_path("v1/after");
        let token = fixture.application_token.clone();
        async move { call(&base, "GET", &path, Some(&token), &[fence_header(4)], b"").await }
    });
    let (stream, _) = read_open(&mut replacement).await;
    respond(&mut replacement, stream, 200, vec![], b"after").await;
    let outcome = request.await.expect("relay request finishes");
    assert_eq!(
        outcome.status, 200,
        "the refused peer must not poison the manager"
    );
    fixture.teardown().await;
}
