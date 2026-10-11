use desk_foreman::runner::service_relay::{StreamRequest, TunnelHandle};
use runner_protocol::service_relay::RelayId;
use runner_protocol::{
    ExportNativeTaskRequest, ExportNativeTaskResult, RunnerOwner,
    native_task_export_output_manifest_sha256, validate_native_task_export_selection,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::export_helpers::{
    export_path, manager_upload, replace_with_ready_successor, schedule_export, selected_content,
    stop_prepared_task, upload_for,
};
use super::native_execution_helpers::poll_task_job;
use super::support::TaskFixture;

#[tokio::test]
async fn a_released_task_exports_exact_selected_files_after_successor_ready() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let stop_job_id = stop_prepared_task(&fixture, task_id).await;
    replace_with_ready_successor(&fixture, stop_job_id, task_id).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    fixture
        .relay
        .attach_tunnel(
            fixture.manager_id,
            TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx),
        )
        .unwrap();
    let stream = fixture
        .relay
        .open_stream(StreamRequest {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            environment_id: fixture.environment_id,
            generation: 5,
            manager_id: fixture.manager_id,
            declared_body_length: 0,
        })
        .unwrap();

    let paths = ["removed.txt", "README.md", "new.txt"];
    let (first, second) = tokio::join!(
        schedule_export(&fixture, task_id, stop_job_id, "export-intent", &paths),
        schedule_export(&fixture, task_id, stop_job_id, "export-intent", &paths),
    );
    assert_eq!(first.status, 200, "{}", first.body_text());
    assert_eq!(second.status, 200, "{}", second.body_text());
    let first: Value = serde_json::from_slice(&first.body).unwrap();
    let second: Value = serde_json::from_slice(&second.body).unwrap();
    assert_eq!(first["job_id"], second["job_id"]);
    assert_eq!(first["stop_job_id"], stop_job_id.to_string());
    assert_eq!(first["state"], "pending");
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            stop_job_id,
            "export-intent",
            &["README.md"]
        )
        .await
        .status,
        409
    );
    assert_eq!(
        schedule_export(&fixture, task_id, stop_job_id, "export-other-token", &paths)
            .await
            .status,
        409
    );

    let (job_id, job) = poll_task_job(&fixture).await;
    assert_eq!(job["kind"], "export_native_task");
    let request: ExportNativeTaskRequest = serde_json::from_value(job["payload"].clone()).unwrap();
    assert!(matches!(
        &request.owner,
        RunnerOwner::ExecutionEnvironment { generation: 4, .. }
    ));
    assert_eq!(request.task.task_id, task_id);
    assert_eq!(request.paths, ["README.md", "new.txt", "removed.txt"]);

    let content = selected_content(&request);
    let total_bytes = content
        .files
        .iter()
        .map(|file| file.content.len())
        .sum::<usize>() as u32;
    assert_eq!(
        fixture
            .manager_result(
                job_id,
                true,
                json!({
                    "task_id": task_id,
                    "stop_job_id": stop_job_id,
                    "selection_sha256": request.selection_sha256,
                    "output_manifest_sha256": "b".repeat(64),
                    "file_count": 2,
                    "missing_count": 1,
                    "total_bytes": total_bytes
                })
            )
            .await
            .status,
        409,
        "generic successful result cannot precede content upload"
    );
    let mut invalid = content.clone();
    invalid.files.pop();
    invalid.output_manifest_sha256 = "0".repeat(64);
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &upload_for(request.clone(), invalid),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        409
    );

    let upload = upload_for(request.clone(), content.clone());
    assert_eq!(
        manager_upload(&fixture, job_id, &upload, None).await.status,
        401
    );
    let (first_upload, second_upload) = tokio::join!(
        manager_upload(&fixture, job_id, &upload, Some(&fixture.manager_token)),
        manager_upload(&fixture, job_id, &upload, Some(&fixture.manager_token)),
    );
    assert_eq!(first_upload.status, 204);
    assert_eq!(second_upload.status, 204);
    assert_eq!(
        manager_upload(&fixture, job_id, &upload, Some(&fixture.manager_token))
            .await
            .status,
        204,
        "exact terminal upload replay is read-only"
    );
    let mut changed = content.clone();
    changed.files[0].content = "different bytes".to_string();
    let selection = validate_native_task_export_selection(&request.paths).unwrap();
    changed.output_manifest_sha256 = native_task_export_output_manifest_sha256(
        &selection,
        &changed.files,
        &changed.missing_paths,
    )
    .unwrap();
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &upload_for(request.clone(), changed),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        409
    );

    let status = fixture
        .get(
            &fixture.application_token,
            &export_path(&fixture, task_id, "/status"),
        )
        .await;
    assert_eq!(status.status, 200, "{}", status.body_text());
    let status: Value = serde_json::from_slice(&status.body).unwrap();
    assert_eq!(status["state"], "succeeded");
    assert_eq!(status["selection_sha256"], request.selection_sha256);
    assert_eq!(
        status["output_manifest_sha256"],
        content.output_manifest_sha256
    );
    let body = fixture
        .get(
            &fixture.application_token,
            &export_path(&fixture, task_id, "/content"),
        )
        .await;
    assert_eq!(body.status, 200, "{}", body.body_text());
    let body: Value = serde_json::from_slice(&body.body).unwrap();
    assert_eq!(body["files"][0]["path"], "README.md");
    assert_eq!(body["files"][1]["path"], "new.txt");
    assert_eq!(body["missing_paths"], json!(["removed.txt"]));
    assert_eq!(
        fixture
            .manager_result(
                job_id,
                true,
                serde_json::to_value(ExportNativeTaskResult {
                    task_id,
                    stop_job_id,
                    selection_sha256: request.selection_sha256,
                    output_manifest_sha256: content.output_manifest_sha256,
                    file_count: 2,
                    missing_count: 1,
                    total_bytes,
                })
                .unwrap()
            )
            .await
            .status,
        204
    );
    let (state, result_json, files): (String, Value, i64) = sqlx::query_as(
        "SELECT job.state, job.result, \
         (SELECT COUNT(*) FROM execution_environment_task_export_files WHERE job_id = job.job_id) \
         FROM execution_environment_jobs AS job WHERE job.job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    assert_eq!(state, "succeeded");
    assert_eq!(files, 2);
    assert!(result_json.get("files").is_none());
    let environment: (i64, String, Option<Uuid>) = sqlx::query_as(
        "SELECT generation, state, active_native_task_id FROM execution_environments WHERE environment_id = $1",
    ).bind(fixture.environment_id).fetch_one(&fixture.database.pool).await.unwrap();
    let export_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_task_exports WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    let cancelled = stream.is_cancelled();
    drop(stream);
    fixture.teardown().await;
    assert_eq!(export_count, 1);
    assert!(
        !cancelled,
        "export mutated the successor generation's stream"
    );
    assert_eq!(environment, (5, "ready".to_string(), None));
}
