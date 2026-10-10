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
//!
//! A running acknowledgment is stronger than "the container reports
//! running" (issue 749 P1). The trusted command and state working
//! directory are validated before any disk or Docker work, and every
//! reuse/start/create branch additionally runs the same-container non-root
//! state probe before it can return `running`. A probe failure is
//! uncertainty about state, not a stop proof: nothing is removed, stopped,
//! or retried automatically, and the durable high-water stays as written.

use runner_protocol::{
    EnsureExecutionEnvironmentAck, EnsureExecutionEnvironmentRequest,
    execution_environment as protocol,
};

use super::ensure_helpers;
use super::file_lock::{EnvFileLock, lock_holder_token, release_and_finish};
use super::inspect::{ensure_owner_matches, inspect_environment_container};
use super::service::ServiceCredentialStore;
use super::state_probe::confirm_state_writable;
use super::tombstone::{TombstoneStore, advance_high_water, ensure_generation_admissible};
use super::{ExecutionEnvironmentProfile, argv};
use crate::runtime::docker::DockerRunnerBackend;

/// Idempotent ensure for one `(environment_id, generation)`.
///
/// Fails closed when the profile is disabled, when volume ownership, the
/// trusted command, or the dedicated state root is not explicit, when the
/// owner is malformed, when a durable barrier already closed the
/// generation, or when another generation holds state. Same-generation
/// running containers return immediately without recreation; any reused
/// container must match the requested owner labels, and any returned
/// acknowledgment must additionally pass the state probe.
pub async fn ensure_execution_environment(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: EnsureExecutionEnvironmentRequest,
) -> anyhow::Result<EnsureExecutionEnvironmentAck> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(|e| anyhow::anyhow!(e))?;
    let environment_id = environment_id.to_string();
    profile.ensure_enabled()?;
    // Trusted process inputs first: an absent or malformed command,
    // working directory, root uid, or private-service profile refuses
    // every ensure branch — including reuse — before any disk or Docker
    // work.
    let process = profile.ensure_process_profile()?;
    let service = profile.ensure_service_profile()?;
    let volume_name =
        protocol::derive_state_volume_name(&environment_id).map_err(|e| anyhow::anyhow!(e))?;
    let state_mount_target = profile.state_mount_target.clone();

    let deadline = profile.operation_deadline()?;
    let state_dir = profile.ensure_state_dir()?;
    let credentials = ServiceCredentialStore::new(state_dir);
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

        let same_generation = ensure_helpers::scan_same_generation_container(
            backend,
            &environment_id,
            generation,
            &request.owner,
            &deadline,
        )
        .await?;

        if let Some(info) = same_generation {
            if info.status == protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE
                || info.status == "running"
            {
                // Reuse: labels already matched above, so probe the real
                // state writability before the idempotent acknowledgment.
                confirm_state_writable(backend, &container_name, &process, &deadline).await?;
                // A running container keeps the credential it was launched
                // with. Require it to still be present and valid rather than
                // minting a new one, which would silently break the agent
                // that is already serving with the old value.
                if let Some(config) = service {
                    credentials.existing(&environment_id, generation, &config.auth_env_name)?;
                }
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
                    confirm_state_writable(backend, &container_name, &process, &deadline).await?;
                    // Restart reuses the same container and therefore the
                    // same credential file; the env was baked in at
                    // creation, so an unusable file means the container
                    // cannot serve and the restart is refused.
                    if let Some(config) = service {
                        credentials.existing(&environment_id, generation, &config.auth_env_name)?;
                    }
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
        // Only this branch may mint a credential: the admission lock is
        // already held, the durable barrier is written, and this is before
        // the container run. Reuse and restart above only read.
        let mut credential_env_file: Option<String> = None;
        if let Some(config) = service {
            let credential =
                credentials.obtain(&environment_id, generation, &config.auth_env_name)?;
            credential_env_file = Some(credential.env_file().to_string_lossy().into_owned());
        }
        let service_run = match (service, credential_env_file.as_deref()) {
            (Some(config), Some(env_file)) => Some(argv::ServiceRunSpec {
                network: &config.network,
                credential_env_file: env_file,
            }),
            _ => None,
        };
        let spec = argv::EnvironmentRunSpec {
            container_name: &container_name,
            owner_stable_key: &owner_stable_key,
            environment_id: &environment_id,
            generation,
            manager_id: &manager_id,
            volume_name: &volume_name,
            state_mount_target: &state_mount_target,
            state_workdir: process.state_workdir,
            command: process.command,
            service: service_run,
            volume_uid: process.uid,
            volume_gid: process.gid,
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
        // Create: the final owner and running-state validation above must
        // be followed by the same-container non-root state probe. A probe
        // failure leaves the container and the durable high-water exactly
        // as they are; nothing is removed or restarted to force a retry.
        confirm_state_writable(backend, &container_name, &process, &deadline).await?;
        Ok(EnsureExecutionEnvironmentAck {
            owner: request.owner,
            container_name,
            state: protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE.to_string(),
        })
    }
    .await;

    release_and_finish(file_lock, outcome)
}
