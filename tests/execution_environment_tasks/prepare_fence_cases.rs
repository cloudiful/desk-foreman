//! Deadline and owner-fence regressions for native preparation acknowledgements.

use uuid::Uuid;

use super::prepare_helpers::{admitted_and_seeded, prepare_job, task_state};
use super::support::{TaskFixture, call};

async fn schedule_and_claim(fixture: &TaskFixture, task_id: Uuid, token: &str) -> Uuid {
    let scheduled = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": token }),
        )
        .await;
    assert_eq!(scheduled.status, 200, "body: {}", scheduled.body_text());
    let (job_id, _, _) = prepare_job(fixture, task_id).await;
    sqlx::query(
        "UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = NOW() WHERE job_id = $1",
    )
    .bind(job_id)
    .execute(&fixture.database.pool)
    .await
    .expect("claim preparation");
    job_id
}

async fn valid_ack(fixture: &TaskFixture, task_id: Uuid) -> serde_json::Value {
    let seed: (String, i64, i64) = sqlx::query_as(
        "SELECT manifest_sha256, file_count, total_bytes \
         FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("seed header");
    serde_json::json!({
        "task_id": task_id,
        "manifest_sha256": seed.0,
        "file_count": seed.1,
        "total_bytes": seed.2,
        "prepared": true,
    })
}

#[tokio::test]
async fn an_expired_claim_cannot_read_its_seed_or_complete() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-expired-seed").await;
    let job_id = schedule_and_claim(&fixture, task_id, "prep-expired-seed").await;
    sqlx::query(
        "UPDATE execution_environment_jobs \
         SET deadline_at = NOW() - INTERVAL '1 second' WHERE job_id = $1",
    )
    .bind(job_id)
    .execute(&fixture.database.pool)
    .await
    .expect("expire claim");

    let seed = call(
        &fixture.base,
        "GET",
        &format!(
            "/api/internal/runner-managers/{}/environment-jobs/{job_id}/task-seed",
            fixture.manager_id
        ),
        Some(&fixture.manager_token),
        &[],
        b"",
    )
    .await;
    assert_eq!(seed.status, 404, "body: {}", seed.body_text());

    let ack = fixture
        .manager_result(job_id, true, valid_ack(&fixture, task_id).await)
        .await;
    assert_eq!(ack.status, 409, "body: {}", ack.body_text());
    assert_eq!(task_state(&fixture, task_id).await, "preparing");
    let (_, state, _) = prepare_job(&fixture, task_id).await;
    assert_eq!(state, "claimed", "the expired ack changed no durable row");
    fixture.teardown().await;
}

#[tokio::test]
async fn an_invalidated_generation_cannot_complete_a_claimed_preparation() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-invalidated").await;
    let job_id = schedule_and_claim(&fixture, task_id, "prep-invalidated").await;
    sqlx::query("UPDATE execution_environments SET generation = 5 WHERE environment_id = $1")
        .bind(fixture.environment_id)
        .execute(&fixture.database.pool)
        .await
        .expect("advance owner fence");

    let ack = fixture
        .manager_result(job_id, true, valid_ack(&fixture, task_id).await)
        .await;
    assert_eq!(ack.status, 409, "body: {}", ack.body_text());
    assert_eq!(task_state(&fixture, task_id).await, "preparing");
    let (_, state, _) = prepare_job(&fixture, task_id).await;
    assert_eq!(state, "claimed");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_cancelled_preparation_rejects_a_late_success_ack() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-cancel-late").await;
    let job_id = schedule_and_claim(&fixture, task_id, "prep-cancel-late").await;
    let canceled = fixture
        .json_post(
            &fixture.application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    assert_eq!(canceled.status, 200, "body: {}", canceled.body_text());
    assert_eq!(task_state(&fixture, task_id).await, "uncertain");

    let ack = fixture
        .manager_result(job_id, true, valid_ack(&fixture, task_id).await)
        .await;
    assert_eq!(ack.status, 409, "body: {}", ack.body_text());
    assert_eq!(task_state(&fixture, task_id).await, "uncertain");
    let (_, state, _) = prepare_job(&fixture, task_id).await;
    assert_eq!(state, "claimed", "a late ack cannot record job success");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_failed_preparation_atomically_retains_uncertain_ownership() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-failed-prepare").await;
    let job_id = schedule_and_claim(&fixture, task_id, "prep-failed").await;
    let outcome = fixture
        .manager_result(job_id, false, serde_json::Value::Null)
        .await;
    assert_eq!(outcome.status, 204, "body: {}", outcome.body_text());
    assert_eq!(task_state(&fixture, task_id).await, "uncertain");
    let (_, state, _) = prepare_job(&fixture, task_id).await;
    assert_eq!(state, "uncertain");
    fixture.teardown().await;
}
