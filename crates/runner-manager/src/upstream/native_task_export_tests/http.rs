use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use desk_foreman::runner::RunnerFuture;
use runner_protocol::*;

use crate::runtime::{ProcessSpawnTarget, RunnerBackend};

struct Backend(ExportNativeTaskUpload, bool);
impl RunnerBackend for Backend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn prepare_shell_spawn<'a>(
        &'a self,
        _: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn run_command<'a>(
        &'a self,
        _: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn cleanup_runner_owner<'a>(&'a self, _: RunnerOwner) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn export_native_task<'a>(
        &'a self,
        request: ExportNativeTaskRequest,
    ) -> RunnerFuture<'a, anyhow::Result<NativeTaskExportContent>> {
        Box::pin(async move {
            assert_eq!(request, self.0.request);
            if self.1 {
                anyhow::bail!("capture refused private-host-path")
            }
            Ok(self.0.content.clone())
        })
    }
}

#[derive(Clone)]
struct Server {
    status: StatusCode,
    uploads: Arc<Mutex<Vec<ExportNativeTaskUpload>>>,
}

async fn accept(
    State(state): State<Server>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> StatusCode {
    assert_eq!(headers["authorization"], "Bearer test-export-token");
    state
        .uploads
        .lock()
        .unwrap()
        .push(serde_json::from_slice(&body).unwrap());
    state.status
}

async fn exercise(
    status: StatusCode,
    refuse: bool,
) -> (
    anyhow::Result<serde_json::Value>,
    Vec<ExportNativeTaskUpload>,
    ExportNativeTaskUpload,
) {
    let upload = super::upload("retained edited text\n".into());
    let backend = Backend(upload.clone(), refuse);
    let job_id = uuid::Uuid::new_v4();
    let state = Server {
        status,
        uploads: Arc::new(Mutex::new(vec![])),
    };
    let router = Router::new()
        .route(
            &format!("/api/internal/runner-managers/7/environment-jobs/{job_id}/task-export"),
            post(accept),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let job = RunnerJob {
        job_id: format!("envjob:{job_id}"),
        kind: "export_native_task".into(),
        payload: serde_json::to_value(&upload.request).unwrap(),
    };
    let result = super::super::execute_job(
        &reqwest::Client::new(),
        &base,
        "test-export-token",
        "7",
        &backend,
        &job,
    )
    .await;
    server.abort();
    let uploads = state.uploads.lock().unwrap().clone();
    (result, uploads, upload)
}

#[tokio::test]
async fn native_task_export_http_204_returns_summary_without_contents() {
    let (result, uploads, upload) = exercise(StatusCode::NO_CONTENT, false).await;
    assert_eq!(uploads, vec![upload.clone()]);
    let value = result.unwrap();
    let summary: ExportNativeTaskResult = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(summary.task_id, upload.request.task.task_id);
    assert_eq!(
        summary.output_manifest_sha256,
        upload.content.output_manifest_sha256
    );
    assert!(value.get("files").is_none());
    assert!(!value.to_string().contains("retained edited text"));
}

#[tokio::test]
async fn native_task_export_http_refusal_never_retries_or_completes() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::CONFLICT,
        StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::OK,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let (result, uploads, _) = exercise(status, false).await;
        assert!(result.is_err());
        assert_eq!(uploads.len(), 1);
        assert_eq!(
            result.unwrap_err().to_string(),
            "native task export upload failed or refused"
        );
    }
}

#[tokio::test]
async fn native_task_export_serialized_oversize_never_sends() {
    let mut upload = super::upload(String::new());
    let files: Vec<_> = (0..8)
        .map(|index| {
            SourceSeedSnapshotFile::new(
                format!("file{index}"),
                "\u{1}".repeat(MAX_SEED_CONTENT_BYTES),
            )
        })
        .collect();
    let selection = validate_native_task_export_selection(
        &files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    upload.request.paths = selection.paths.clone();
    upload.request.selection_sha256 = selection.selection_sha256.clone();
    upload.content.selection_sha256 = selection.selection_sha256.clone();
    upload.content.output_manifest_sha256 =
        native_task_export_output_manifest_sha256(&selection, &files, &[]).unwrap();
    upload.content.files = files;
    validate_native_task_export_content(&upload.request, &upload.content).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let job = RunnerJob {
        job_id: format!("envjob:{}", uuid::Uuid::new_v4()),
        kind: "export_native_task".into(),
        payload: serde_json::to_value(&upload.request).unwrap(),
    };
    let result = super::super::execute_job(
        &reqwest::Client::new(),
        &base,
        "test-export-token",
        "7",
        &Backend(upload, false),
        &job,
    )
    .await;
    assert!(result.is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_task_export_upload_disconnect_is_not_success() {
    use tokio::io::AsyncReadExt;
    let upload = super::upload("text".into());
    let backend = Backend(upload.clone(), false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        // Drop the connection without an upload acknowledgement.
    });
    let job = RunnerJob {
        job_id: format!("envjob:{}", uuid::Uuid::new_v4()),
        kind: "export_native_task".into(),
        payload: serde_json::to_value(&upload.request).unwrap(),
    };
    let result = super::super::execute_job(
        &reqwest::Client::new(),
        &base,
        "test-export-token",
        "7",
        &backend,
        &job,
    )
    .await;
    server.await.unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn native_task_export_capture_refusal_never_uploads_and_redacts() {
    let (result, uploads, _) = exercise(StatusCode::NO_CONTENT, true).await;
    assert!(uploads.is_empty());
    assert_eq!(
        result.unwrap_err().to_string(),
        "native task export upload failed or refused"
    );
}
