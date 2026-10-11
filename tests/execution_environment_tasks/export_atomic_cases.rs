use runner_protocol::ExportNativeTaskRequest;
use uuid::Uuid;

use super::export_helpers::{
    manager_upload, schedule_export, selected_content, stop_prepared_task, upload_for,
};
use super::native_execution_helpers::poll_task_job;
use super::support::TaskFixture;

#[tokio::test]
async fn export_output_and_success_state_roll_back_together() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let stop_job_id = stop_prepared_task(&fixture, task_id).await;
    let scheduled = schedule_export(
        &fixture,
        task_id,
        stop_job_id,
        "export-atomic",
        &["README.md"],
    )
    .await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let (job_id, job) = poll_task_job(&fixture).await;
    let request: ExportNativeTaskRequest = serde_json::from_value(job["payload"].clone()).unwrap();
    let content = selected_content(&request);
    sqlx::query(
        "CREATE FUNCTION reject_task_export_success() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.job_kind = 'export_native_task' AND NEW.state = 'succeeded' THEN \
         RAISE EXCEPTION 'injected export completion failure'; END IF; RETURN NEW; END $$",
    )
    .execute(&fixture.database.pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_task_export_success BEFORE UPDATE ON execution_environment_jobs \
         FOR EACH ROW EXECUTE FUNCTION reject_task_export_success()",
    )
    .execute(&fixture.database.pool)
    .await
    .unwrap();
    let failed = manager_upload(
        &fixture,
        job_id,
        &upload_for(request.clone(), content.clone()),
        Some(&fixture.manager_token),
    )
    .await;
    let after_failure: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT job.state, export.output_manifest_sha256, \
         (SELECT COUNT(*) FROM execution_environment_task_export_files WHERE job_id = job.job_id) \
         FROM execution_environment_jobs AS job \
         JOIN execution_environment_task_exports AS export ON export.job_id = job.job_id \
         WHERE job.job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    sqlx::query("DROP TRIGGER reject_task_export_success ON execution_environment_jobs")
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_task_export_success()")
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    let retry = manager_upload(
        &fixture,
        job_id,
        &upload_for(request, content),
        Some(&fixture.manager_token),
    )
    .await;
    fixture.teardown().await;
    assert_eq!(failed.status, 500, "{}", failed.body_text());
    assert_eq!(after_failure, ("claimed".to_string(), None, 0));
    assert_eq!(retry.status, 204, "{}", retry.body_text());
}
