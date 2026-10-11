use reqwest::StatusCode;
use uuid::Uuid;

use super::support::{
    dispatch_export, export_file_count, export_header_count, prepared, schedule_export, stop,
};

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory native-task export gate"]
async fn native_task_export_wrong_proof_and_expired_upload_store_no_content() {
    let rig = prepared(&[("README.md", "retained\n")]).await;
    let stop_job_id = stop(&rig, "export-stop-refusal").await;

    let wrong_proof =
        schedule_export(&rig, Uuid::new_v4(), "export-wrong-proof", &["README.md"]).await;
    assert_eq!(wrong_proof.status(), StatusCode::CONFLICT);
    assert_eq!(export_header_count(&rig).await, 0);

    let scheduled =
        schedule_export(&rig, stop_job_id, "export-expired-upload", &["README.md"]).await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let job = rig.poll("export_native_task").await;
    sqlx::query(
        "UPDATE execution_environment_jobs \
         SET deadline_at = clock_timestamp() - INTERVAL '1 second' WHERE job_id = $1",
    )
    .bind(super::support::wire_uuid(&job))
    .execute(&rig.fixture.db.pool)
    .await
    .expect("expire claimed upload gate");

    let result = dispatch_export(&rig, job.clone()).await;
    assert!(!result.ok);
    assert_eq!(
        result.error.as_deref(),
        Some("native task export upload failed or refused")
    );
    assert_eq!(export_file_count(&rig, &job).await, 0);
    let download = rig
        .fixture
        .client
        .get(format!(
            "{}/api/internal/execution-environments/{}/tasks/{}/export/content",
            rig.fixture.base, rig.env_id, rig.task_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .send()
        .await
        .expect("refused export remains unavailable");
    assert_eq!(download.status(), StatusCode::CONFLICT);
    rig.fixture.db.teardown().await;
}
