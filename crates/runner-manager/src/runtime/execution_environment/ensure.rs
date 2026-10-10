//! Idempotent environment ensure (issue 739 P1, bounded in 740 P1,
//! durable-barriered in 742 P1).
//!
//! Serializes on the environment id, refuses creation when any other
//! generation is running or unknown, and never deletes a container to
//! force an ensure through. Every success acknowledgment requires
//! matching owner/environment/generation labels, and every Docker command
//! runs inside one monotonic operation deadline. Docker label scans are a
//! guard, not an atomic distributed lock: multiple managers still need
//! durable affinity/CAS in a later phase.
//!
//! A dedicated persistent state root adds a durable, cross-process layer:
//! under a per-environment file reservation the manager refuses a
//! closed/lower generation from the durable barrier before any Docker
//! command, then persists this attempt so a later late ensure cannot run a
//! lower generation either. The reservation is released only on an
//! ordinary operation exit, so a crash or cancellation leaves it for
//! explicit operator recovery.

use runner_protocol::{
    EnsureExecutionEnvironmentAck, EnsureExecutionEnvironmentRequest,
    execution_environment as protocol,
};

use super::file_lock::{EnvFileLock, lock_holder_token, release_and_finish};
use super::inspect::{EnvContainerInfo, ensure_owner_matches, inspect_environment_container};
use super::tombstone::{TombstoneStore, advance_high_water, ensure_generation_admissible};
use super::{ExecutionEnvironmentProfile, LABEL_ENVIRONMENT, argv};
use crate::runtime::docker::DockerRunnerBackend;

/// Idempotent ensure for one `(environment_id, generation)`.
///
/// Fails closed when the profile is disabled, when volume ownership or the
/// dedicated state root is not explicit, when the owner is malformed, when
/// a durable barrier already closed the generation, or when another
/// generation holds state. Same-generation running containers return
/// immediately without recreation; any reused container must match the
/// requested owner labels before it can be acknowledged.
pub async fn ensure_execution_environment(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: EnsureExecutionEnvironmentRequest,
) -> anyhow::Result<EnsureExecutionEnvironmentAck> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(|e| anyhow::anyhow!(e))?;
    let environment_id = environment_id.to_string();
    profile.ensure_enabled()?;
    let (volume_uid, volume_gid) = profile.ensure_volume_ownership()?;
    let volume_name =
        protocol::derive_state_volume_name(&environment_id).map_err(|e| anyhow::anyhow!(e))?;

    let deadline = profile.operation_deadline()?;
    let state_dir = profile.ensure_state_dir()?;
    // Gate the dedicated-root filesystem work on the aggregate deadline
    // before it starts instead of hiding it in an async outer timeout.
    deadline.remaining_ms()?;
    let store = TombstoneStore::open(state_dir, &environment_id)?;

    let lock = backend.environment_runner_lock(&environment_id);
    let lock_budget = deadline.remaining()?;
    let _guard = tokio::time::timeout(lock_budget, lock.lock())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out acquiring execution environment {environment_id} lock within the operation deadline"
            )
        })?;

    // Cross-process reservation. The file lock serializes disk read/modify
    // and the whole Docker operation, so two cooperating managers sharing
    // one state root cannot ensure/stop the same environment at once.
    deadline.remaining_ms()?;
    let manager_id = backend.manager_id.clone();
    let file_lock = EnvFileLock::acquire(&store.lock_path(), lock_holder_token(&manager_id))?;

    let outcome: anyhow::Result<EnsureExecutionEnvironmentAck> = async move {
        // Durable generation fence: refuse a closed or lower generation
        // before any Docker command, then persist this attempt so a later
        // late ensure cannot reach Docker either. Every filesystem stage is
        // gated by the shared aggregate deadline before it starts; the
        // operation is never hidden in an async outer timeout.
        deadline.remaining_ms()?;
        let barrier = store.load()?;
        ensure_generation_admissible(&barrier, generation)?;
        store.save(advance_high_water(barrier, generation))?;
        deadline.remaining_ms()?;

        // Durable state volume only; task workspaces stay unmounted this
        // phase. Invoked through the fake harness in tests, never as a live
        // CLI here.
        let volume_argv = argv::build_volume_create_argv(&volume_name, &environment_id);
        let volume_output = backend
            .docker_output_owned(volume_argv, Some(deadline.remaining_ms()?), None)
            .await?;
        if volume_output.timed_out {
            anyhow::bail!(
                "docker volume create timed out within the execution environment operation deadline"
            );
        }
        if volume_output.exit_code != Some(0) {
            anyhow::bail!(
                "docker failed to create environment volume: exit_code={:?} output={}",
                volume_output.exit_code,
                volume_output.output.trim()
            );
        }

        let container_name = request.owner.container_name();
        let owner_stable_key = request.owner.stable_key();

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
            if info.environment_id.as_deref() != Some(environment_id.as_str()) {
                continue;
            }
            match info.generation {
                Some(other) if other == generation => {
                    // Same generation must belong to this exact owner before
                    // it can be reused or acknowledged.
                    ensure_owner_matches(&info, &request.owner)?;
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

        if let Some(info) = same_generation {
            if info.status == protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE
                || info.status == "running"
            {
                return Ok(EnsureExecutionEnvironmentAck {
                    owner: request.owner,
                    container_name,
                    state: protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE.to_string(),
                });
            }
            if protocol::is_stopped_status(&info.status) {
                backend
                    .docker_status_bounded(
                        vec!["start".to_string(), container_name.clone()],
                        deadline.remaining_ms()?,
                    )
                    .await?;
                let rechecked =
                    inspect_environment_container(backend, &container_name, deadline.remaining_ms()?)
                        .await?
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "environment container {container_name} disappeared after start"
                            )
                        })?;
                ensure_owner_matches(&rechecked, &request.owner)?;
                if rechecked.status == "running" {
                    return Ok(EnsureExecutionEnvironmentAck {
                        owner: request.owner,
                        container_name,
                        state: protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE.to_string(),
                    });
                }
                anyhow::bail!(
                    "environment container {container_name} is {} after start; refusing idempotent success",
                    rechecked.status
                );
            } else {
                anyhow::bail!(
                    "execution environment {environment_id} generation {generation} is {}; refusing creation",
                    info.status
                );
            }
        }

        let (config_snapshot, manager_id) = {
            let config = backend.config.read().await.clone();
            (config, backend.manager_id.clone())
        };
        let spec = argv::EnvironmentRunSpec {
            container_name: &container_name,
            owner_stable_key: &owner_stable_key,
            environment_id: &environment_id,
            generation,
            manager_id: &manager_id,
            volume_name: &volume_name,
            state_mount_target: &profile.state_mount_target,
            volume_uid,
            volume_gid,
            config: &config_snapshot,
        };
        let run_argv = argv::build_environment_run_argv(&spec);
        let run_output = backend
            .docker_output_owned(run_argv, Some(deadline.remaining_ms()?), None)
            .await?;
        if run_output.timed_out {
            anyhow::bail!(
                "docker run timed out within the execution environment operation deadline"
            );
        }
        if run_output.exit_code != Some(0) {
            anyhow::bail!(
                "docker failed to create environment container: exit_code={:?} output={}",
                run_output.exit_code,
                run_output.output.trim()
            );
        }
        let created =
            inspect_environment_container(backend, &container_name, deadline.remaining_ms()?)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "environment container {container_name} could not be inspected after creation"
                    )
                })?;
        ensure_owner_matches(&created, &request.owner)?;
        if created.status != "running" {
            backend
                .docker_status_bounded(
                    vec!["start".to_string(), container_name.clone()],
                    deadline.remaining_ms()?,
                )
                .await?;
            let restarted =
                inspect_environment_container(backend, &container_name, deadline.remaining_ms()?)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "environment container {container_name} disappeared after start"
                        )
                    })?;
            ensure_owner_matches(&restarted, &request.owner)?;
            if restarted.status != "running" {
                anyhow::bail!(
                    "environment container {container_name} is {} after creation; refusing success",
                    restarted.status
                );
            }
        }
        Ok(EnsureExecutionEnvironmentAck {
            owner: request.owner,
            container_name,
            state: protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE.to_string(),
        })
    }
    .await;

    release_and_finish(file_lock, outcome)
}
