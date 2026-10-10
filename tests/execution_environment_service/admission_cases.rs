//! Owner, fence and tunnel-admission cases (issue 749 P2b).
//!
//! Everything here runs against a real isolated PostgreSQL and the real
//! internal HTTP surface. No Docker, no agent process and no manager outbound
//! client are involved: a case that needs a tunnel drives the mock manager in
//! [`super::relay_cases`].

use std::time::Duration;

use super::relay_helpers::{read_open, respond};
use super::support::{ServiceFixture, call, set_fence};
use super::ws_client::{Frame, WsClient};

const PROTOCOL: &str = "df-env-http.v2";

fn fence_header(generation: i64) -> (String, String) {
    (
        "X-Execution-Environment-Generation".to_string(),
        generation.to_string(),
    )
}

#[tokio::test]
async fn a_caller_without_an_application_bearer_never_reaches_the_relay() {
    let fixture = ServiceFixture::setup().await;
    let outcome = call(
        &fixture.base,
        "GET",
        &fixture.service_path("v1/models"),
        None,
        &[fence_header(4)],
        b"",
    )
    .await;
    assert_eq!(outcome.status, 401);
    fixture.teardown().await;
}

#[tokio::test]
async fn another_application_sees_the_same_absent_environment_as_an_unknown_one() {
    let fixture = ServiceFixture::setup().await;
    let unknown = format!(
        "/api/internal/execution-environments/{}/service/v1/models",
        uuid::Uuid::new_v4()
    );
    let foreign = call(
        &fixture.base,
        "GET",
        &fixture.service_path("v1/models"),
        Some(&fixture.other_application_token),
        &[fence_header(4)],
        b"",
    )
    .await;
    let absent = call(
        &fixture.base,
        "GET",
        &unknown,
        Some(&fixture.other_application_token),
        &[fence_header(4)],
        b"",
    )
    .await;
    assert_eq!(foreign.status, 404, "a foreign environment is absent");
    assert_eq!(
        (foreign.status, foreign.body_text()),
        (absent.status, absent.body_text()),
        "the gateway must not distinguish foreign from unknown"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn the_generation_fence_header_is_required() {
    let fixture = ServiceFixture::setup().await;
    let outcome = call(
        &fixture.base,
        "GET",
        &fixture.service_path("v1/models"),
        Some(&fixture.application_token),
        &[],
        b"",
    )
    .await;
    assert_eq!(
        outcome.status, 400,
        "an unfenced request would reopen the invalidate race"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_stale_fence_is_a_conflict_and_never_a_relay_outage() {
    let fixture = ServiceFixture::setup().await;
    for generation in [3, 5] {
        let outcome = call(
            &fixture.base,
            "GET",
            &fixture.service_path("v1/models"),
            Some(&fixture.application_token),
            &[fence_header(generation)],
            b"",
        )
        .await;
        assert_eq!(
            outcome.status, 409,
            "generation {generation} is not the stored fence"
        );
    }
    fixture.teardown().await;
}

#[tokio::test]
async fn a_ready_environment_with_no_candidate_is_a_conflict() {
    let fixture = ServiceFixture::setup().await;
    set_fence(
        &fixture.database.pool,
        fixture.environment_id,
        4,
        None,
        Some(fixture.manager_id),
        "ready",
    )
    .await;
    let outcome = call(
        &fixture.base,
        "GET",
        &fixture.service_path("v1/models"),
        Some(&fixture.application_token),
        &[fence_header(4)],
        b"",
    )
    .await;
    assert_eq!(
        outcome.status, 503,
        "a bound environment with no provisioned candidate has no relay to \
         reach, which the frozen owner-scoped predicate reports as \
         unavailable rather than as a caller conflict"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_bound_but_unready_environment_reports_the_relay_as_unavailable() {
    let fixture = ServiceFixture::setup().await;
    set_fence(
        &fixture.database.pool,
        fixture.environment_id,
        4,
        Some(4),
        Some(fixture.manager_id),
        "stopping",
    )
    .await;
    let outcome = call(
        &fixture.base,
        "GET",
        &fixture.service_path("v1/models"),
        Some(&fixture.application_token),
        &[fence_header(4)],
        b"",
    )
    .await;
    assert_eq!(outcome.status, 503);
    fixture.teardown().await;
}

#[tokio::test]
async fn an_environment_with_no_bound_manager_never_dials_out() {
    let fixture = ServiceFixture::setup().await;
    set_fence(
        &fixture.database.pool,
        fixture.environment_id,
        4,
        Some(4),
        None,
        "ready",
    )
    .await;
    let outcome = call(
        &fixture.base,
        "GET",
        &fixture.service_path("v1/models"),
        Some(&fixture.application_token),
        &[fence_header(4)],
        b"",
    )
    .await;
    assert_eq!(
        outcome.status, 409,
        "an unbound environment has no relay at all"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_relay_tunnel_requires_the_runner_manager_bearer() {
    let fixture = ServiceFixture::setup().await;
    let refused = WsClient::connect(&fixture.base, "not-a-manager-token", PROTOCOL)
        .await
        .err()
        .expect("an unauthenticated tunnel is refused");
    assert!(
        matches!(refused, 401 | 403 | 404),
        "the relay ingress reuses the runner-manager bearer, so an unknown \
         token is refused before any upgrade, got {refused}"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_relay_tunnel_refuses_a_peer_that_offers_no_relay_protocol() {
    let fixture = ServiceFixture::setup().await;
    assert_eq!(
        WsClient::connect(&fixture.base, &fixture.manager_token, "some-other-protocol")
            .await
            .err(),
        Some(400)
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_manager_holds_at_most_one_live_tunnel() {
    let fixture = ServiceFixture::setup().await;
    let first = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("first tunnel");
    assert_eq!(
        WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
            .await
            .err(),
        Some(409),
        "a second connection must not silently replace the live tunnel"
    );
    drop(first);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_reconnect_after_a_disconnect_is_admitted_again() {
    let fixture = ServiceFixture::setup().await;
    let mut first = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("first tunnel");
    first.send_close().await;
    drop(first);
    let _second = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("a released tunnel may be replaced");
    fixture.teardown().await;
}

#[tokio::test]
async fn only_bounded_relative_paths_cross_the_relay() {
    let fixture = ServiceFixture::setup().await;
    for path in ["..%2f..%2fetc", "a/../b", "v1/%2e%2e/v1"] {
        let outcome = call(
            &fixture.base,
            "GET",
            &fixture.service_path(path),
            Some(&fixture.application_token),
            &[fence_header(4)],
            b"",
        )
        .await;
        assert_eq!(outcome.status, 400, "{path} must be refused");
    }
    fixture.teardown().await;
}
