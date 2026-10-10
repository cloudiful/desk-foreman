//! Seed retrieval and dispatch for native-task preparation (issue 753 P2b).
//!
//! The manager fetches a job's sealed seed from the authenticated control plane
//! and materializes it through the backend. Two rules shape this module:
//!
//! - **Bounds before decode.** The response body is read with a hard byte cap
//!   while it streams, so an oversized or unlabelled body is refused before any
//!   JSON is parsed. `Content-Length` is never trusted on its own.
//! - **Same rules, one implementation.** The decoded entries are re-validated
//!   and re-hashed with the shared `runner_protocol::task_seed` helper, and the
//!   result must equal the identity the job announced. A mismatch fails before
//!   any filesystem write.
//!
//! Nothing here logs seed content, and no path or subject from the response is
//! used to build a filesystem path: the layout comes from the trusted job
//! identity alone.

use anyhow::{Context, bail};
use futures_util::StreamExt;
use reqwest::Client;
use runner_protocol::{
    NativeTaskPreparedResult, PrepareNativeTaskRequest, SourceSeedSnapshotFile,
    validate_seed_bundle,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::runtime::RunnerBackend;

/// Hard cap on the seed response body, applied while streaming.
const MAX_SEED_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Reserved wire-id prefix for durable environment jobs.
const ENVJOB_PREFIX: &str = "envjob:";

/// The bounded seed content the control plane returns.
#[derive(Debug, Deserialize)]
struct NativeTaskSeedContent {
    manifest_sha256: String,
    file_count: u32,
    total_bytes: u32,
    files: Vec<SourceSeedSnapshotFile>,
}

/// Dispatch one durable native-task preparation job.
///
/// The seed is fetched over the manager's own bearer and validated with the
/// shared rules; the backend then does the filesystem work. Any failure here is
/// reported as a failed result, which the control plane turns into an
/// `uncertain` task — never a false success.
pub(crate) async fn execute_job(
    client: &Client,
    base_url: &str,
    token: &str,
    manager_id: &str,
    backend: &dyn RunnerBackend,
    job: &runner_protocol::RunnerJob,
) -> anyhow::Result<serde_json::Value> {
    let request = serde_json::from_value::<PrepareNativeTaskRequest>(job.payload.clone())?;
    let result = prepare(
        client,
        base_url,
        token,
        manager_id,
        &job.job_id,
        request,
        backend,
    )
    .await?;
    Ok(serde_json::to_value(result)?)
}

/// Fetch, validate and materialize one preparation.
///
/// `job_id` is the polled wire id; the route is derived from the configured
/// base URL and the manager's own registered id, never from the payload.
pub(crate) async fn prepare(
    client: &Client,
    base_url: &str,
    token: &str,
    manager_id: &str,
    job_id: &str,
    request: PrepareNativeTaskRequest,
    backend: &dyn RunnerBackend,
) -> anyhow::Result<NativeTaskPreparedResult> {
    let uuid = parse_wire_id(job_id)?;
    let numeric_id: i64 = manager_id.parse().map_err(|_| {
        anyhow::anyhow!("this manager has no registered control-plane identity yet")
    })?;
    let body = fetch_body(client, base_url, token, numeric_id, uuid).await?;
    let bundle = decode_and_validate(&body, &request)?;
    backend.prepare_native_task(request, bundle).await
}

/// Parse the durable wire id carried by a polled job.
pub(crate) fn parse_wire_id(job_id: &str) -> anyhow::Result<Uuid> {
    let raw = job_id
        .strip_prefix(ENVJOB_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("native task preparation requires a durable job id"))?;
    Uuid::parse_str(raw).map_err(|_| anyhow::anyhow!("durable native task job id is not a UUID"))
}

/// Fetch the seed body with a streaming cap.
///
/// The cap is enforced chunk by chunk, so a body that exceeds it is refused
/// even when the server advertises no length or a false one.
async fn fetch_body(
    client: &Client,
    base_url: &str,
    token: &str,
    manager_id: i64,
    job_id: Uuid,
) -> anyhow::Result<Vec<u8>> {
    let url = format!(
        "{}/api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-seed",
        base_url.trim_end_matches('/')
    );
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .context("native task seed request failed")?;
    if !response.status().is_success() {
        bail!(
            "control plane refused the native task seed ({})",
            response.status()
        );
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("native task seed stream failed")?;
        if body.len() + chunk.len() > MAX_SEED_RESPONSE_BYTES {
            bail!("native task seed response exceeds the response cap");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Decode and validate a fetched seed body against the announced identity.
///
/// Split out so the bounds/digest rules are testable without a live control
/// plane: this is the exact function a live fetch feeds.
pub(crate) fn decode_and_validate(
    body: &[u8],
    request: &PrepareNativeTaskRequest,
) -> anyhow::Result<runner_protocol::SeedBundle> {
    if body.len() > MAX_SEED_RESPONSE_BYTES {
        bail!("native task seed response exceeds the response cap");
    }
    let content: NativeTaskSeedContent = serde_json::from_slice(body)
        .map_err(|error| anyhow::anyhow!("native task seed response is not valid JSON: {error}"))?;
    let bundle = validate_seed_bundle(&content.files)
        .map_err(|error| anyhow::anyhow!("native task seed is invalid: {}", error.code()))?;
    if bundle.manifest_sha256 != content.manifest_sha256
        || bundle.file_count != content.file_count
        || bundle.total_bytes != content.total_bytes
    {
        bail!("native task seed response metadata does not match its contents");
    }
    if bundle.manifest_sha256 != request.manifest_sha256
        || bundle.file_count != request.file_count
        || bundle.total_bytes != request.total_bytes
    {
        bail!("native task seed does not match the announced job identity");
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runner_protocol::{RunnerOwner, validate_seed_bundle};

    fn request_for(files: &[(&str, &str)]) -> PrepareNativeTaskRequest {
        let entries: Vec<SourceSeedSnapshotFile> = files
            .iter()
            .map(|(path, content)| SourceSeedSnapshotFile::new(*path, *content))
            .collect();
        let bundle = validate_seed_bundle(&entries).expect("bundle");
        PrepareNativeTaskRequest {
            owner: RunnerOwner::ExecutionEnvironment {
                environment_id: Uuid::new_v4().to_string(),
                generation: 1,
            },
            task_id: Uuid::new_v4(),
            workspace_binding_id: 1,
            manifest_sha256: bundle.manifest_sha256,
            file_count: bundle.file_count,
            total_bytes: bundle.total_bytes,
        }
    }

    #[test]
    fn a_matching_body_decodes_to_the_same_bundle() {
        let request = request_for(&[("a.txt", "a"), ("b.txt", "b")]);
        let body = serde_json::json!({
            "manifest_sha256": request.manifest_sha256,
            "file_count": request.file_count,
            "total_bytes": request.total_bytes,
            "files": [ { "path": "a.txt", "content": "a" }, { "path": "b.txt", "content": "b" } ],
        });
        let bundle = decode_and_validate(body.to_string().as_bytes(), &request).expect("decode");
        assert_eq!(bundle.file_count, 2);
        assert_eq!(bundle.manifest_sha256, request.manifest_sha256);
    }

    #[test]
    fn a_body_that_disagrees_with_the_job_identity_is_refused() {
        let request = request_for(&[("a.txt", "a")]);
        let body = serde_json::json!({
            "manifest_sha256": "0".repeat(64),
            "file_count": 1,
            "total_bytes": 1,
            "files": [ { "path": "a.txt", "content": "a" } ],
        });
        assert!(decode_and_validate(body.to_string().as_bytes(), &request).is_err());
    }

    #[test]
    fn an_oversized_body_is_refused() {
        let request = request_for(&[]);
        let body = vec![b' '; MAX_SEED_RESPONSE_BYTES + 1];
        assert!(decode_and_validate(&body, &request).is_err());
    }

    #[test]
    fn a_body_with_unknown_fields_is_refused() {
        let request = request_for(&[("a.txt", "a")]);
        let body = serde_json::json!({
            "manifest_sha256": request.manifest_sha256,
            "file_count": request.file_count,
            "total_bytes": request.total_bytes,
            "files": [ { "path": "a.txt", "content": "a", "mode": "0755" } ],
        });
        assert!(decode_and_validate(body.to_string().as_bytes(), &request).is_err());
    }

    #[test]
    fn a_wire_id_must_carry_the_durable_prefix() {
        let id = Uuid::new_v4();
        assert_eq!(parse_wire_id(&format!("envjob:{id}")).expect("parse"), id);
        assert!(parse_wire_id("plain-id").is_err());
    }
}
