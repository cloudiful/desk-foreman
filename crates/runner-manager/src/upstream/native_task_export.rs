use std::io::Write;

#[cfg(test)]
#[path = "native_task_export_tests.rs"]
mod tests;

use anyhow::bail;
use reqwest::{Client, StatusCode};
use runner_protocol::{
    ExportNativeTaskRequest, ExportNativeTaskResult, ExportNativeTaskUpload,
    MAX_TASK_EXPORT_UPLOAD_BODY_BYTES, RunnerJob,
};

use crate::runtime::RunnerBackend;

pub(super) async fn execute_job(
    client: &Client,
    base_url: &str,
    token: &str,
    manager_id: &str,
    backend: &dyn RunnerBackend,
    job: &RunnerJob,
) -> anyhow::Result<serde_json::Value> {
    execute(client, base_url, token, manager_id, backend, job)
        .await
        .map_err(|_| anyhow::anyhow!("native task export upload failed or refused"))
}

async fn execute(
    client: &Client,
    base_url: &str,
    token: &str,
    manager_id: &str,
    backend: &dyn RunnerBackend,
    job: &RunnerJob,
) -> anyhow::Result<serde_json::Value> {
    let job_id = super::native_task::parse_wire_id(&job.job_id)?;
    let manager_id: i64 = manager_id.parse()?;
    if manager_id <= 0 {
        bail!("invalid export manager identity");
    }
    let request: ExportNativeTaskRequest = serde_json::from_value(job.payload.clone())?;
    let content = backend.export_native_task(request.clone()).await?;
    let checked = runner_protocol::validate_native_task_export_content(&request, &content)
        .map_err(|_| anyhow::anyhow!("invalid captured export"))?;
    let summary = ExportNativeTaskResult {
        task_id: content.task_id,
        stop_job_id: content.stop_job_id,
        selection_sha256: content.selection_sha256.clone(),
        output_manifest_sha256: content.output_manifest_sha256.clone(),
        file_count: checked.file_count,
        missing_count: checked.missing_count,
        total_bytes: checked.total_bytes,
    };
    let bytes = bounded_body(&ExportNativeTaskUpload {
        request,
        content: checked.content,
    })?;
    let url = format!(
        "{}/api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-export",
        base_url.trim_end_matches('/'),
    );
    // One immutable body and one send: no retry or recapture after any response.
    let response = client
        .post(url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await?;
    if response.status() != StatusCode::NO_CONTENT {
        bail!("export upload was not acknowledged");
    }
    Ok(serde_json::to_value(summary)?)
}

struct BoundedBody(Vec<u8>);

impl Write for BoundedBody {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_TASK_EXPORT_UPLOAD_BODY_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("export upload exceeds transport cap"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn bounded_body(upload: &ExportNativeTaskUpload) -> anyhow::Result<Vec<u8>> {
    let mut body = BoundedBody(Vec::new());
    serde_json::to_writer(&mut body, upload)?;
    Ok(body.0)
}
