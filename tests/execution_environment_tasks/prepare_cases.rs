//! Native-task preparation scheduling and behaviour (issue 753 P2b).
//!
//! Drives the real internal HTTP surface against an isolated PostgreSQL. This
//! file covers the durable intent: its typed native-task link, the `preparing`
//! transition, idempotent replay, the scheduling fences, and the typed
//! completion that marks the task `prepared`.

use uuid::Uuid;

use super::prepare_helpers::{admitted_and_seeded, prepare_job, task_state};
use super::support::TaskFixture;

#[tokio::test]
async fn scheduling_records_a_durable_job_and_moves_the_task_to_preparing() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-prepare").await;

    let outcome = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": "prep-1" }),
        )
        .await;
    assert_eq!(outcome.status, 200, "body: {}", outcome.body_text());
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(body["task_id"], task_id.to_string());
    assert_eq!(body["task_state"], "preparing");
    assert_eq!(body["job_state"], "pending");
    assert_eq!(body["target_generation"], 4);
    assert_eq!(body["active"], true);
    assert_eq!(body["released"], false);
    // The descriptor exposes identity/status only.
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "active",
            "job_id",
            "job_state",
            "released",
            "target_generation",
            "task_id",
            "task_state"
        ]
    );

    let (job_id, state, native_task_id) = prepare_job(&fixture, task_id).await;
    assert_eq!(state, "pending");
    assert_eq!(native_task_id, Some(task_id));
    assert_eq!(body["job_id"], job_id.to_string());
    assert_eq!(task_state(&fixture, task_id).await, "preparing");
    // The task links the job, in the same commit.
    let linked: (Option<Uuid>,) =
        sqlx::query_as("SELECT prepare_job_id FROM execution_environment_tasks WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("link");
    assert_eq!(linked.0, Some(job_id));
    fixture.teardown().await;
}

#[tokio::test]
async fn an_identical_replay_returns_the_same_job() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-replay").await;
    let first = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": "prep-replay" }),
        )
        .await;
    assert_eq!(first.status, 200);
    let second = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": "prep-replay" }),
        )
        .await;
    assert_eq!(second.status, 200, "body: {}", second.body_text());
    assert_eq!(
        first.body, second.body,
        "a replay returns the same descriptor"
    );

    // A different token against an existing preparation is a conflict.
    let conflict = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": "prep-other" }),
        )
        .await;
    assert_eq!(conflict.status, 409);

    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_jobs \
         WHERE native_task_id = $1 AND job_kind = 'prepare_native_task'",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count");
    assert_eq!(count.0, 1, "exactly one durable preparation");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_missing_seed_refuses_scheduling() {
    // One active task per environment, so the unseeded case needs its own
    // fixture.
    let fixture = TaskFixture::setup().await;
    let unseeded = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(unseeded, "op-unseeded").await.status,
        200
    );
    assert_eq!(
        fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(unseeded),
                4,
                serde_json::json!({ "expected_generation": 4, "operation_token": "prep-a" }),
            )
            .await
            .status,
        409,
        "a task with no sealed seed cannot be prepared"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_stale_generation_fence_refuses_scheduling() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-stale").await;
    assert_eq!(
        fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(task_id),
                99,
                serde_json::json!({ "expected_generation": 99, "operation_token": "prep-b" }),
            )
            .await
            .status,
        409
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_disabled_original_manager_refuses_scheduling_with_503() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-manager").await;
    sqlx::query("UPDATE runner_managers SET enabled = FALSE WHERE runner_manager_id = $1")
        .bind(fixture.manager_id)
        .execute(&fixture.database.pool)
        .await
        .expect("disable manager");

    let outcome = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": "prep-m" }),
        )
        .await;
    assert_eq!(outcome.status, 503, "body: {}", outcome.body_text());
    // Nothing was enqueued.
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_jobs WHERE native_task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count");
    assert_eq!(count.0, 0);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_matching_prepared_ack_marks_the_task_prepared_once() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-ack").await;
    assert_eq!(
        fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(task_id),
                4,
                serde_json::json!({ "expected_generation": 4, "operation_token": "prep-ack" }),
            )
            .await
            .status,
        200
    );
    let (job_id, _, _) = prepare_job(&fixture, task_id).await;
    // Claim it as the manager would.
    sqlx::query("UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = NOW() WHERE job_id = $1")
        .bind(job_id)
        .execute(&fixture.database.pool)
        .await
        .expect("claim job");

    let seed: (String, i64, i64) = sqlx::query_as(
        "SELECT manifest_sha256, file_count, total_bytes FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("seed header");

    let ack = fixture
        .manager_result(
            job_id,
            true,
            serde_json::json!({
                "task_id": task_id,
                "manifest_sha256": seed.0,
                "file_count": seed.1,
                "total_bytes": seed.2,
                "prepared": true,
            }),
        )
        .await;
    assert_eq!(ack.status, 204, "body: {}", ack.body_text());
    assert_eq!(task_state(&fixture, task_id).await, "prepared");
    // The environment readiness is never touched by a preparation.
    let generation: (Option<i64>,) = sqlx::query_as(
        "SELECT container_generation FROM execution_environments WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("environment");
    assert_eq!(generation.0, None, "a preparation never sets a candidate");
    fixture.teardown().await;
}
