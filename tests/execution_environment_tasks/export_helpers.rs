use runner_protocol::{
    ExportNativeTaskRequest, ExportNativeTaskUpload, NativeTaskExportContent, RunnerOwner,
    SourceSeedSnapshotFile, native_task_export_output_manifest_sha256,
    validate_native_task_export_selection,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::native_execution_helpers::{poll_task_job, prepare_task, start_result, stop_result};
use super::support::{TaskFixture, call};

pub(super) async fn stop_prepared_task(fixture: &TaskFixture, task_id: Uuid) -> Uuid {
    prepare_task(fixture, task_id).await;
    let start = fixture
        .json_post(
            &fixture.application_token,
            &format!("{}/start", fixture.task_path(task_id)),
            4,
            json!({"expected_generation": 4, "operation_token": "export-start"}),
        )
        .await;
    assert_eq!(start.status, 200, "{}", start.body_text());
    let (start_id, start_job) = poll_task_job(fixture).await;
    assert_eq!(start_job["kind"], "start_native_task");
    assert_eq!(
        fixture
            .manager_result(start_id, true, start_result(&start_job))
            .await
            .status,
        204
    );
    let stop = fixture
        .json_post(
            &fixture.application_token,
            &format!("{}/stop", fixture.task_path(task_id)),
            4,
            json!({"expected_generation": 4, "operation_token": "export-stop"}),
        )
        .await;
    assert_eq!(stop.status, 200, "{}", stop.body_text());
    let (stop_id, stop_job) = poll_task_job(fixture).await;
    assert_eq!(stop_job["kind"], "stop_native_task");
    assert_eq!(
        fixture
            .manager_result(stop_id, true, stop_result(&stop_job, true))
            .await
            .status,
        204
    );
    stop_id
}

pub(super) async fn replace_with_ready_successor(
    fixture: &TaskFixture,
    stop_job_id: Uuid,
    old_task_id: Uuid,
) {
    assert_eq!(
        fixture.release(old_task_id, stop_job_id, 5, 4).await.status,
        200
    );
    let replaced = fixture
        .json_post(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/replace",
                fixture.environment_id
            ),
            5,
            json!({
                "expected_generation": 5,
                "expected_container_generation": 4,
                "expected_stop_job_id": stop_job_id,
                "operation_token": "export-successor-replace"
            }),
        )
        .await;
    assert_eq!(replaced.status, 200, "{}", replaced.body_text());
    let (job_id, job) = poll_task_job(fixture).await;
    let owner: RunnerOwner = serde_json::from_value(job["payload"]["owner"].clone()).unwrap();
    assert_eq!(
        fixture
            .manager_result(
                job_id,
                true,
                json!({
                    "owner": owner,
                    "container_name": owner.container_name(),
                    "state": "running"
                }),
            )
            .await
            .status,
        204
    );
    let status = fixture
        .get(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/status",
                fixture.environment_id
            ),
        )
        .await;
    assert_eq!(status.status, 200, "{}", status.body_text());
    let status: Value = serde_json::from_slice(&status.body).unwrap();
    assert_eq!(status["state"], "ready");
    assert_eq!(status["generation"], 5);
}

pub(super) fn export_path(fixture: &TaskFixture, task_id: Uuid, suffix: &str) -> String {
    format!("{}/export{suffix}", fixture.task_path(task_id))
}

pub(super) async fn schedule_export(
    fixture: &TaskFixture,
    task_id: Uuid,
    stop_job_id: Uuid,
    token: &str,
    paths: &[&str],
) -> super::support::HttpOutcome {
    call(
        &fixture.base,
        "POST",
        &export_path(fixture, task_id, ""),
        Some(&fixture.application_token),
        &[],
        serde_json::to_vec(&json!({
            "operation_token": token,
            "stop_job_id": stop_job_id,
            "paths": paths,
        }))
        .unwrap()
        .as_slice(),
    )
    .await
}

pub(super) fn upload_for(
    request: ExportNativeTaskRequest,
    content: NativeTaskExportContent,
) -> Vec<u8> {
    serde_json::to_vec(&ExportNativeTaskUpload { request, content }).unwrap()
}

pub(super) fn selected_content(request: &ExportNativeTaskRequest) -> NativeTaskExportContent {
    let mut files = Vec::new();
    let mut missing_paths = Vec::new();
    for path in &request.paths {
        match path.as_str() {
            "README.md" => files.push(SourceSeedSnapshotFile::new(path.clone(), "retained edit")),
            "new.txt" => files.push(SourceSeedSnapshotFile::new(path.clone(), "")),
            _ => missing_paths.push(path.clone()),
        }
    }
    let selection = validate_native_task_export_selection(&request.paths).unwrap();
    let output_manifest_sha256 =
        native_task_export_output_manifest_sha256(&selection, &files, &missing_paths).unwrap();
    NativeTaskExportContent {
        task_id: request.task.task_id,
        stop_job_id: request.stop_job_id,
        selection_sha256: request.selection_sha256.clone(),
        output_manifest_sha256,
        files,
        missing_paths,
    }
}

pub(super) async fn manager_upload(
    fixture: &TaskFixture,
    job_id: Uuid,
    body: &[u8],
    token: Option<&str>,
) -> super::support::HttpOutcome {
    let path = format!(
        "/api/internal/runner-managers/{}/environment-jobs/{job_id}/task-export",
        fixture.manager_id
    );
    call(
        &fixture.base,
        "POST",
        &path,
        token,
        &[("Content-Type".to_string(), "application/json".to_string())],
        body,
    )
    .await
}
