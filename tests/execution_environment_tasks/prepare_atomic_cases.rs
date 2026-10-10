//! Atomic commit/rollback checks for manager preparation acknowledgements.

use super::prepare_helpers::{admitted_and_seeded, prepare_job, task_state};
use super::support::TaskFixture;

#[tokio::test]
async fn a_task_update_failure_rolls_back_the_job_success() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-atomic-ack").await;
    let scheduled = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({
                "expected_generation": 4,
                "operation_token": "prep-atomic-ack",
            }),
        )
        .await;
    assert_eq!(scheduled.status, 200, "body: {}", scheduled.body_text());
    let (job_id, _, _) = prepare_job(&fixture, task_id).await;
    sqlx::query(
        "UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = NOW() WHERE job_id = $1",
    )
    .bind(job_id)
    .execute(&fixture.database.pool)
    .await
    .expect("claim preparation");
    let seed: (String, i64, i64) = sqlx::query_as(
        "SELECT manifest_sha256, file_count, total_bytes \
         FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("seed header");

    sqlx::query(
        "CREATE FUNCTION reject_prepared_task_update() RETURNS trigger \
         LANGUAGE plpgsql AS $$ BEGIN \
         IF NEW.state = 'prepared' THEN RAISE EXCEPTION 'injected task update failure'; END IF; \
         RETURN NEW; END $$",
    )
    .execute(&fixture.database.pool)
    .await
    .expect("create failure trigger function");
    sqlx::query(
        "CREATE TRIGGER reject_prepared_task_update \
         BEFORE UPDATE OF state ON execution_environment_tasks \
         FOR EACH ROW EXECUTE FUNCTION reject_prepared_task_update()",
    )
    .execute(&fixture.database.pool)
    .await
    .expect("create failure trigger");

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
    assert_eq!(ack.status, 500, "body: {}", ack.body_text());

    let job_state: (String,) =
        sqlx::query_as("SELECT state FROM execution_environment_jobs WHERE job_id = $1")
            .bind(job_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("job state");
    assert_eq!(job_state.0, "claimed", "job success must roll back");
    assert_eq!(task_state(&fixture, task_id).await, "preparing");
    fixture.teardown().await;
}
