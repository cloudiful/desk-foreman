use std::time::Duration;

use runner_protocol::{ExportNativeTaskRequest, SourceSeedSnapshotFile};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use super::export_helpers::{
    export_path, manager_upload, schedule_export, selected_content, stop_prepared_task, upload_for,
};
use super::http_client::split_base;
use super::native_execution_helpers::poll_task_job;
use super::support::{TaskFixture, call};

async fn oversized_chunked_upload(fixture: &TaskFixture, job_id: Uuid) -> u16 {
    let (host, port) = split_base(&fixture.base);
    let mut stream = tokio::net::TcpStream::connect((host.as_str(), port))
        .await
        .unwrap();
    let header = format!(
        "POST /api/internal/runner-managers/{}/environment-jobs/{job_id}/task-export HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\nAuthorization: Bearer {}\r\n\r\n",
        fixture.manager_id, fixture.manager_token,
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    let body = vec![b'x'; runner_protocol::MAX_TASK_EXPORT_UPLOAD_BODY_BYTES + 1];
    let _ = stream
        .write_all(format!("{:x}\r\n", body.len()).as_bytes())
        .await;
    let _ = stream.write_all(&body).await;
    let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut response))
        .await
        .unwrap();
    String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn export_selection_shape_and_count_are_bounded_before_persistence() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    for paths in [
        vec![],
        vec!["../escape"],
        vec![".git/config"],
        vec!["a", "a"],
        vec!["a", "a/b"],
    ] {
        assert_eq!(
            schedule_export(&fixture, task_id, proof_id, "invalid-selection", &paths)
                .await
                .status,
            400
        );
    }
    let long = "a".repeat(513);
    assert_eq!(
        schedule_export(&fixture, task_id, proof_id, "long-selection", &[&long])
            .await
            .status,
        400
    );
    let paths = (0..4097)
        .map(|index| format!("f{index}.txt"))
        .collect::<Vec<_>>();
    let refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        schedule_export(&fixture, task_id, proof_id, "many-selection", &refs)
            .await
            .status,
        413
    );
    let unknown = call(&fixture.base, "POST", &export_path(&fixture, task_id, ""), Some(&fixture.application_token), &[],
        &serde_json::to_vec(&json!({"operation_token": "unknown-selection", "stop_job_id": proof_id, "paths": ["README.md"], "host_path": "/etc"})).unwrap()).await;
    assert_eq!(unknown.status, 400);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_environment_task_exports")
        .fetch_one(&fixture.database.pool)
        .await
        .unwrap();
    fixture.teardown().await;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn export_upload_enforces_text_content_and_transport_limits() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    let paths = (0..9)
        .map(|index| format!("f{index}.txt"))
        .collect::<Vec<_>>();
    let refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        schedule_export(&fixture, task_id, proof_id, "bounded-upload", &refs)
            .await
            .status,
        200
    );
    let (job_id, job) = poll_task_job(&fixture).await;
    let request: ExportNativeTaskRequest = serde_json::from_value(job["payload"].clone()).unwrap();
    let mut content = selected_content(&request);
    content.files = vec![SourceSeedSnapshotFile::new(
        "f0.txt",
        "x".repeat(1024 * 1024 + 1),
    )];
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &upload_for(request.clone(), content.clone()),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        413
    );
    content.files = paths
        .iter()
        .map(|path| SourceSeedSnapshotFile::new(path.clone(), "x".repeat(1024 * 1024)))
        .collect();
    content.missing_paths.clear();
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &upload_for(request.clone(), content.clone()),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        413
    );
    content.files = vec![SourceSeedSnapshotFile::new("f0.txt", "nul\0text")];
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &upload_for(request.clone(), content),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        400
    );
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &vec![b'x'; runner_protocol::MAX_TASK_EXPORT_UPLOAD_BODY_BYTES + 1],
            Some(&fixture.manager_token)
        )
        .await
        .status,
        413
    );
    assert_eq!(oversized_chunked_upload(&fixture, job_id).await, 413);
    let mut unknown = serde_json::to_value(runner_protocol::ExportNativeTaskUpload {
        request: request.clone(),
        content: selected_content(&request),
    })
    .unwrap();
    unknown["host_path"] = json!("/etc");
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &serde_json::to_vec(&unknown).unwrap(),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        400
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_task_export_files WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    fixture.teardown().await;
    assert_eq!(count, 0);
}
