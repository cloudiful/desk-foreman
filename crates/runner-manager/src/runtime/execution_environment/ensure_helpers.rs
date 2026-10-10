//! Focused ensure-path helpers (issue 749 P1).
//!
//! Holds the existing-container survey that decide whether an ensure may
//! reuse an already-running writer or must refuse. The scan is a guard,
//! not a lock: it never deletes anything, and a blocking writer is only
//! ever reported so the caller can fail closed and leave termination to the
//! owner-verified stop path.

use runner_protocol::{RunnerOwner, execution_environment as protocol};

use super::LABEL_ENVIRONMENT;
use super::deadline::OperationDeadline;
use super::inspect::{EnvContainerInfo, ensure_owner_matches, inspect_environment_container};
use crate::runtime::docker::DockerRunnerBackend;

/// Survey the environment's labelled containers and resolve the
/// same-generation writer, if any.
///
/// A same-generation container is returned only after its
/// owner/environment/generation labels match the requested owner. Any
/// other generation that is still running (or whose status is unknown)
/// refuses the operation: stopped generations are ignored, and nothing is
/// ever deleted to force the ensure through. `Ok(None)` means no
/// same-generation container exists, so the caller creates one.
pub(super) async fn scan_same_generation_container(
    backend: &DockerRunnerBackend,
    environment_id: &str,
    generation: i64,
    owner: &RunnerOwner,
    deadline: &OperationDeadline,
) -> anyhow::Result<Option<EnvContainerInfo>> {
    let filter = format!("label={LABEL_ENVIRONMENT}={environment_id}");
    let ps_output = backend
        .docker_output_bounded(
            vec![
                "ps".to_string(),
                "-a".to_string(),
                "--filter".to_string(),
                filter,
                "--format".to_string(),
                "{{.Names}}".to_string(),
            ],
            deadline.remaining_ms()?,
        )
        .await?;
    if ps_output.timed_out {
        anyhow::bail!(
            "docker ps timed out for execution environment {environment_id} within the operation deadline"
        );
    }
    if ps_output.exit_code != Some(0) {
        anyhow::bail!(
            "docker ps failed for execution environment {environment_id}: {}",
            ps_output.output.trim()
        );
    }
    let mut same_generation: Option<EnvContainerInfo> = None;
    for line in ps_output.stdout.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let Some(info) =
            inspect_environment_container(backend, name, deadline.remaining_ms()?).await?
        else {
            continue;
        };
        if info.environment_id.as_deref() != Some(environment_id) {
            continue;
        }
        match info.generation {
            Some(other) if other == generation => {
                // Same generation must belong to this exact owner before
                // it can be reused or acknowledged.
                ensure_owner_matches(&info, owner)?;
                same_generation = Some(info);
            }
            _ => {
                // Other (or malformed) generation: refuse when it may still
                // hold state. Stopped generations are ignored; running or
                // unknown blocks creation. Never delete to force through.
                if protocol::is_running_or_unknown_status(&info.status) {
                    anyhow::bail!(
                        "execution environment {environment_id} generation {generation} refused: generation {:?} is {}",
                        info.generation,
                        info.status
                    );
                }
            }
        }
    }
    Ok(same_generation)
}
