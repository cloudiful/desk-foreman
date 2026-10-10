//! Legacy broker regression and wire-namespace cases (issue 742 P2).
//!
//! Proves a plain-UUID legacy job still round-trips through the unchanged
//! in-memory broker, and that reserved `envjob:` ids never fall back to it:
//! an unknown reserved id is a durable 404 and a malformed reserved id is a
//! durable 400, while a plain unknown UUID still reaches the legacy broker.

use std::sync::Arc;

use serde_json::json;

use desk_foreman::runner::{PullRunnerService, RunnerService};

use super::helpers::test_state;
use super::http::{http_get, http_post, spawn_http};
use super::support::{DispatchDb, create_manager_with_token};

const NEXT: &str = "/api/internal/runner-manager/jobs/next";
const RESULT: &str = "/api/internal/runner-manager/jobs/result";

#[tokio::test]
async fn plain_uuid_legacy_job_still_round_trips_through_the_broker() {
    let db = DispatchDb::setup().await;
    let (_, token) = create_manager_with_token(&db.pool, "dispatch-legacy-mgr").await;
    let state = test_state(db.pool.clone());
    let broker = Arc::clone(&state.runner_broker);
    let (base, handle) = spawn_http(state).await;

    // An authenticated call registers manager liveness, which the legacy
    // broker requires before it will dispatch a job.
    let (status, _) = http_get(&base, "/api/internal/runner-manager/config", Some(&token)).await;
    assert_eq!(status, 200);

    let service = PullRunnerService::new(broker);
    let pending = tokio::spawn(async move { service.list_sessions().await });

    let (status, job) = http_get(&base, NEXT, Some(&token)).await;
    assert_eq!(status, 200);
    let job_id = job["job_id"].as_str().expect("legacy job id").to_string();
    assert!(
        !job_id.starts_with("envjob:"),
        "legacy ids are plain UUIDs, got {job_id}"
    );
    assert_eq!(job["kind"], "list_sessions");

    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        json!({"job_id": job_id, "ok": true, "result": [], "error": null}),
    )
    .await;
    assert_eq!(status, 204, "a legacy result must route to the broker");

    let completed = pending
        .await
        .expect("join legacy job")
        .expect("legacy job result");
    assert!(completed.is_empty());

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn reserved_result_ids_never_fall_back_to_legacy() {
    let db = DispatchDb::setup().await;
    let (_, token) = create_manager_with_token(&db.pool, "dispatch-namespace-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    // A well-formed reserved id with no row is a durable 404, not the legacy
    // broker's unknown-job 500.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        json!({
            "job_id": "envjob:00000000-0000-0000-0000-000000000000",
            "ok": true,
            "result": {},
            "error": null,
        }),
    )
    .await;
    assert_eq!(status, 404, "an unknown reserved id must be a durable 404");

    // A malformed reserved id is rejected as a durable 400.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        json!({"job_id": "envjob:not-a-uuid", "ok": true, "result": {}, "error": null}),
    )
    .await;
    assert_eq!(status, 400, "a malformed reserved id must be a durable 400");

    // A plain unknown UUID is a legacy id and still reaches the broker.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        json!({
            "job_id": "11111111-1111-1111-1111-111111111111",
            "ok": true,
            "result": {},
            "error": null,
        }),
    )
    .await;
    assert_eq!(status, 500, "a plain UUID must route to the legacy broker");

    handle.abort();
    db.teardown().await;
}
