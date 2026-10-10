//! Shared environment-container inspection and owner guard (issue 740 P1).
//!
//! `ensure_owner_matches` is the single owner/environment/generation
//! verification used by ensure reuse and post-create/post-start checks and
//! by stop preflight and final inspection. Inspection is bounded by the
//! caller's remaining operation budget and fails closed: timeouts and
//! transport/parse/permission failures never become absence, and absence
//! is only ever the known `No such container/object` error.

use runner_protocol::{RunnerOwner, execution_environment as protocol};

use super::{LABEL_ENVIRONMENT, LABEL_GENERATION};
use crate::runtime::docker::DockerRunnerBackend;
use crate::runtime::docker_command::is_missing_container_error;

#[derive(Clone, Debug)]
pub(crate) struct EnvContainerInfo {
    pub(crate) status: String,
    pub(crate) owner_key: Option<String>,
    pub(crate) environment_id: Option<String>,
    pub(crate) generation: Option<i64>,
}

/// Guard shared by reuse, post-create/post-start, stop preflight and the
/// final stop inspection: an environment container may only be
/// acknowledged or destructively acted on when its owner, environment id
/// and generation labels all match the requested owner. Missing,
/// malformed or foreign labels refuse.
pub(crate) fn ensure_owner_matches(
    info: &EnvContainerInfo,
    owner: &RunnerOwner,
) -> anyhow::Result<()> {
    let (environment_id, generation) =
        protocol::as_execution_environment(owner).ok_or_else(|| {
            anyhow::anyhow!("execution environment container owner is not an execution environment")
        })?;
    let expected_owner_key = owner.stable_key();
    let matches = info.owner_key.as_deref() == Some(expected_owner_key.as_str())
        && info.environment_id.as_deref() == Some(environment_id)
        && info.generation == Some(generation);
    if matches {
        Ok(())
    } else {
        anyhow::bail!(
            "environment container labels do not match the requested owner; refusing to touch it"
        )
    }
}

/// Inspect one environment container within the remaining operation
/// budget. Timeouts and transport/parse/permission failures fail closed;
/// absence is only ever the known `No such container/object` error.
pub(crate) async fn inspect_environment_container(
    backend: &DockerRunnerBackend,
    container_name: &str,
    timeout_ms: u64,
) -> anyhow::Result<Option<EnvContainerInfo>> {
    let args = vec![
        "inspect".to_string(),
        container_name.to_string(),
        "--format".to_string(),
        "{{json .}}".to_string(),
    ];
    let output = backend.docker_output_bounded(args, timeout_ms).await?;
    if output.timed_out {
        anyhow::bail!(
            "docker inspect timed out for {container_name} within the execution environment operation deadline"
        );
    }
    if output.exit_code != Some(0) {
        let message = if output.stderr.trim().is_empty() {
            output.output.as_str()
        } else {
            output.stderr.as_str()
        };
        if is_missing_container_error(message) {
            return Ok(None);
        }
        anyhow::bail!(
            "docker inspect failed for {container_name}: {}",
            message.trim()
        );
    }
    let value: serde_json::Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        anyhow::anyhow!("failed to parse docker inspect metadata for {container_name}: {error}")
    })?;
    let status = value
        .get("State")
        .and_then(|s| s.get("Status"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let labels = value.get("Config").and_then(|c| c.get("Labels"));
    let get_label = |key: &str| {
        labels
            .and_then(|l| l.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let generation = get_label(LABEL_GENERATION).and_then(|raw| raw.parse::<i64>().ok());
    Ok(Some(EnvContainerInfo {
        status,
        owner_key: get_label(super::LABEL_OWNER),
        environment_id: get_label(LABEL_ENVIRONMENT),
        generation,
    }))
}
