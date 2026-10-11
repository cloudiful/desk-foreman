use serde_json::json;
use uuid::Uuid;

use super::native_execution_helpers::{poll_task_job, prepare_task, start_result, stop_result};
use super::support::TaskFixture;

async fn schedule(fixture: &TaskFixture, task_id: Uuid, operation: &str) {
    let outcome = fixture
        .json_post(
            &fixture.application_token,
            &format!("{}/{operation}", fixture.task_path(task_id)),
            4,
            json!({"expected_generation": 4, "operation_token": format!("reviewer-{operation}")}),
        )
        .await;
    assert_eq!(outcome.status, 200, "{}", outcome.body_text());
}

async fn state(
    fixture: &TaskFixture,
    task_id: Uuid,
    job_id: Uuid,
) -> (String, String, String, Option<Uuid>) {
    sqlx::query_as(
        "SELECT task.state, env.state, job.state, env.stop_proof_job_id \
         FROM execution_environment_tasks AS task \
         JOIN execution_environments AS env ON env.environment_id = task.environment_id \
         JOIN execution_environment_jobs AS job ON job.job_id = $2 WHERE task.task_id = $1",
    )
    .bind(task_id)
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("task environment and job state")
}

#[tokio::test]
async fn native_start_and_stop_completion_roll_back_all_anchors_on_job_write_failure() {
    for operation in ["start", "stop"] {
        let fixture = TaskFixture::setup().await;
        let task_id = Uuid::new_v4();
        prepare_task(&fixture, task_id).await;
        schedule(&fixture, task_id, operation).await;
        let (job_id, job) = poll_task_job(&fixture).await;
        let before = state(&fixture, task_id, job_id).await;
        sqlx::query(
            "CREATE FUNCTION reject_native_success() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF NEW.state = 'succeeded' THEN RAISE EXCEPTION 'reviewer injected job failure'; \
             END IF; RETURN NEW; END $$",
        )
        .execute(&fixture.database.pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_native_success BEFORE UPDATE ON execution_environment_jobs \
             FOR EACH ROW EXECUTE FUNCTION reject_native_success()",
        )
        .execute(&fixture.database.pool)
        .await
        .unwrap();
        let result = if operation == "start" {
            start_result(&job)
        } else {
            stop_result(&job, true)
        };
        let outcome = fixture.manager_result(job_id, true, result).await;
        let after = state(&fixture, task_id, job_id).await;
        fixture.teardown().await;
        assert_eq!(outcome.status, 500, "{}", outcome.body_text());
        assert_eq!(
            after, before,
            "{operation} changed anchors despite rollback"
        );
    }
}

#[tokio::test]
async fn expired_native_claims_racing_status_cannot_publish_ready_or_stop_proof() {
    for operation in ["start", "stop"] {
        let fixture = TaskFixture::setup().await;
        let task_id = Uuid::new_v4();
        prepare_task(&fixture, task_id).await;
        schedule(&fixture, task_id, operation).await;
        let (job_id, job) = poll_task_job(&fixture).await;
        sqlx::query("UPDATE execution_environment_jobs SET deadline_at = NOW() - INTERVAL '1 second' WHERE job_id = $1")
            .bind(job_id)
            .execute(&fixture.database.pool)
            .await
            .unwrap();
        let result = if operation == "start" {
            start_result(&job)
        } else {
            stop_result(&job, true)
        };
        let status_path = format!(
            "/api/internal/execution-environments/{}/status",
            fixture.environment_id
        );
        let (ack, status) = tokio::join!(
            fixture.manager_result(job_id, true, result),
            fixture.get(&fixture.application_token, &status_path),
        );
        let after = state(&fixture, task_id, job_id).await;
        fixture.teardown().await;
        assert_eq!(ack.status, 409, "{}", ack.body_text());
        assert_eq!(status.status, 200, "{}", status.body_text());
        assert_eq!(
            after,
            (
                "uncertain".into(),
                "uncertain".into(),
                "uncertain".into(),
                None
            )
        );
    }
}
