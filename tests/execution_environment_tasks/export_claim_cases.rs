use runner_protocol::ExportNativeTaskRequest;
use serde_json::{Value, json};
use uuid::Uuid;

use super::export_helpers::{
    export_path, manager_upload, replace_with_ready_successor, schedule_export, selected_content,
    stop_prepared_task, upload_for,
};
use super::native_execution_helpers::poll_task_job;
use super::support::TaskFixture;

#[tokio::test]
async fn failed_export_is_terminal_without_output_and_cannot_be_reselected() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            proof_id,
            "export-failure",
            &["README.md"]
        )
        .await
        .status,
        200
    );
    let (job_id, _) = poll_task_job(&fixture).await;
    assert_eq!(
        fixture
            .manager_result(job_id, false, Value::Null)
            .await
            .status,
        204
    );
    assert_eq!(
        fixture
            .manager_result(job_id, false, Value::Null)
            .await
            .status,
        204
    );
    let replay = schedule_export(
        &fixture,
        task_id,
        proof_id,
        "export-failure",
        &["README.md"],
    )
    .await;
    assert_eq!(replay.status, 200);
    let replay: Value = serde_json::from_slice(&replay.body).unwrap();
    assert_eq!(replay["state"], "failed");
    assert_eq!(replay["output_manifest_sha256"], Value::Null);
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            proof_id,
            "export-reselection",
            &["new.txt"]
        )
        .await
        .status,
        409
    );
    assert_eq!(
        fixture
            .get(
                &fixture.application_token,
                &export_path(&fixture, task_id, "/content")
            )
            .await
            .status,
        409
    );
    let stored: Value =
        sqlx::query_scalar("SELECT result FROM execution_environment_jobs WHERE job_id = $1")
            .bind(job_id)
            .fetch_one(&fixture.database.pool)
            .await
            .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_task_export_files WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    fixture.teardown().await;
    assert_eq!(stored, json!({"outcome": "native_task_export_failed"}));
    assert_eq!(count, 0);
}

#[tokio::test]
async fn pending_or_expired_upload_cannot_commit_and_expiry_leaves_successor_ready() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    replace_with_ready_successor(&fixture, proof_id, task_id).await;
    assert_eq!(
        schedule_export(&fixture, task_id, proof_id, "export-expiry", &["README.md"])
            .await
            .status,
        200
    );
    let (job_id, job) = poll_task_job(&fixture).await;
    let request: ExportNativeTaskRequest = serde_json::from_value(job["payload"].clone()).unwrap();
    let upload = upload_for(request.clone(), selected_content(&request));
    sqlx::query("UPDATE execution_environment_jobs SET state = 'pending', claimed_at = NULL WHERE job_id = $1")
        .bind(job_id)
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    assert_eq!(
        manager_upload(&fixture, job_id, &upload, Some(&fixture.manager_token))
            .await
            .status,
        409
    );
    sqlx::query("UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = clock_timestamp(), deadline_at = clock_timestamp() - INTERVAL '1 second' WHERE job_id = $1")
        .bind(job_id).execute(&fixture.database.pool).await.unwrap();
    assert_eq!(
        manager_upload(&fixture, job_id, &upload, Some(&fixture.manager_token))
            .await
            .status,
        409
    );
    let expired = desk_foreman::db::execution_environment_jobs::expire_execution_environment_jobs(
        &fixture.database.pool,
        fixture.application_id,
        fixture.environment_id,
    )
    .await
    .unwrap();
    assert!(
        expired
            .iter()
            .any(|job| job.job_id == job_id && job.state == "uncertain")
    );
    let status = fixture
        .get(
            &fixture.application_token,
            &export_path(&fixture, task_id, "/status"),
        )
        .await;
    assert_eq!(status.status, 200);
    let status: Value = serde_json::from_slice(&status.body).unwrap();
    assert_eq!(status["state"], "uncertain");
    let environment: (i64, String, Option<Uuid>) = sqlx::query_as(
        "SELECT generation, state, active_native_task_id FROM execution_environments WHERE environment_id = $1",
    ).bind(fixture.environment_id).fetch_one(&fixture.database.pool).await.unwrap();
    let task_state: String =
        sqlx::query_scalar("SELECT state FROM execution_environment_tasks WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&fixture.database.pool)
            .await
            .unwrap();
    let output_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_task_export_files WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    fixture.teardown().await;
    assert_eq!(environment, (5, "ready".to_string(), None));
    assert_eq!(task_state, "released");
    assert_eq!(output_count, 0);
}
