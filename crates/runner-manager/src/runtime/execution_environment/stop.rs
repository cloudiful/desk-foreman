//! Confirmed environment stop (issue 739 P1, bounded/owner-checked in 740
//! P1, durable-barriered in 742 P1).
//!
//! Stop no longer requires creation to be enabled: an existing environment
//! must stay stoppable even when new creation is disabled. After deriving
//! the owner it acquires the same durable per-environment reservation,
//! records the stop intent in the durable barrier before any destructive
//! command, and only then inspects/terminates. A positively absent container
//! confirms without any destructive command; a present container is only
//! touched when its owner/environment/generation labels match, and a
//! matching container that is already stopped confirms without destruction.
//! A running matching container is terminated with a bounded stop plus
//! `rm -f` escalation (which kills native descendants) inside one monotonic
//! operation deadline, then inspected again. Never checks `active_ops` as a
//! substitute for termination, never removes the durable volume, and never
//! treats transport, parse, timeout, or permission failures as absence.
//!
//! Persisted stop intent and actual stop acknowledgment stay distinct: the
//! barrier is written before the destructive step, so a failed or timed-out
//! stop still closes the old generation to a late ensure while never
//! returning `confirmed=true`.

use runner_protocol::{
    StopExecutionEnvironmentAck, StopExecutionEnvironmentRequest, execution_environment as protocol,
};
use tokio::time::timeout;

use super::ExecutionEnvironmentProfile;
use super::deadline::OperationDeadline;
use super::file_lock::{EnvFileLock, lock_holder_token, release_and_finish};
use super::inspect::{ensure_owner_matches, inspect_environment_container};
use super::tombstone::{TombstoneStore, record_stopped};
use crate::runtime::docker::DockerRunnerBackend;
use crate::runtime::docker_command::is_missing_container_error;

/// Confirmed stop for one exact generation.
///
/// Returns `confirmed=true` only after a successful inspect proves
/// `exited`/`dead`/`removed` or positively identified absence. All other
/// outcomes are `Err` (transport/parse/timeout/permission) or
/// `confirmed=false` (still running/unknown) with no replacement implied.
pub async fn stop_execution_environment(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: StopExecutionEnvironmentRequest,
) -> anyhow::Result<StopExecutionEnvironmentAck> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(|e| anyhow::anyhow!(e))?;
    let environment_id = environment_id.to_string();

    // Identity validation and hardening stay; only the ensure-enabled
    // precondition is dropped so disabled creation cannot strand an
    // existing environment.
    let deadline = profile.operation_deadline()?;
    let state_dir = profile.ensure_state_dir()?;
    deadline.remaining_ms()?;
    let store = TombstoneStore::open(state_dir, &environment_id)?;

    let lock = backend.environment_runner_lock(&environment_id);
    let lock_budget = deadline.remaining()?;
    let _guard = timeout(lock_budget, lock.lock()).await.map_err(|_| {
        anyhow::anyhow!(
            "timed out acquiring execution environment {environment_id} lock within the operation deadline"
        )
    })?;

    deadline.remaining_ms()?;
    let manager_id = backend.manager_id.clone();
    let file_lock = EnvFileLock::acquire(&store.lock_path(), lock_holder_token(&manager_id))?;

    let outcome: anyhow::Result<StopExecutionEnvironmentAck> = async move {
        // Fail closed on corrupt/invalid durable state before any Docker
        // call, then reuse the loaded barrier for the stop record. The
        // filesystem stage is gated by the shared aggregate deadline.
        deadline.remaining_ms()?;
        let barrier = store.load()?;

        let container_name = request.owner.container_name();

        // Preflight: positively absent confirms with no destructive command;
        // a present container is only touched after its labels match.
        let preflight =
            inspect_environment_container(backend, &container_name, deadline.remaining_ms()?)
                .await?;
        if let Some(info) = &preflight {
            ensure_owner_matches(info, &request.owner)?;
        }

        // Durable stop intent before any destructive command or
        // confirmation. A later ensure of this generation is now refused
        // even if the termination below fails or the process restarts.
        store.save(record_stopped(barrier, generation))?;

        let Some(info) = preflight else {
            return Ok(StopExecutionEnvironmentAck {
                owner: request.owner,
                container_name,
                confirmed: true,
                observed_state: protocol::EXECUTION_ENVIRONMENT_ABSENT_STATE.to_string(),
            });
        };

        // A matching, already-stopped container is terminal: confirm without
        // running stop/rm. Anything running-like or unknown is terminated.
        if !protocol::is_stopped_status(&info.status) {
            stop_and_remove_within(backend, &container_name, profile, &deadline).await?;
        }

        // Positive confirmation only. Missing via the known Docker error is
        // absence; any other failure never becomes absence.
        let observed =
            inspect_environment_container(backend, &container_name, deadline.remaining_ms()?)
                .await?;
        match observed {
            None => Ok(StopExecutionEnvironmentAck {
                owner: request.owner,
                container_name,
                confirmed: true,
                observed_state: protocol::EXECUTION_ENVIRONMENT_ABSENT_STATE.to_string(),
            }),
            Some(info) => {
                ensure_owner_matches(&info, &request.owner)?;
                Ok(StopExecutionEnvironmentAck {
                    owner: request.owner,
                    container_name,
                    confirmed: protocol::is_stopped_status(&info.status),
                    observed_state: info.status,
                })
            }
        }
    }
    .await;

    release_and_finish(file_lock, outcome)
}

/// Bounded `stop` plus `rm`/`rm -f` escalation for a verified owner.
///
/// The graceful stop is bounded by `min(stop_timeout, remaining)`; removal
/// and force removal consume the remaining aggregate budget. Missing at
/// any step is quiescent. Every command reuses the bounded command runner,
/// so a deadline kills and reaps the Docker CLI instead of orphaning it.
async fn stop_and_remove_within(
    backend: &DockerRunnerBackend,
    container_name: &str,
    profile: &ExecutionEnvironmentProfile,
    deadline: &OperationDeadline,
) -> anyhow::Result<()> {
    let stop_budget = deadline.remaining_ms()?.min(profile.stop_timeout_ms());
    match backend
        .docker_status_bounded(
            vec!["stop".to_string(), container_name.to_string()],
            stop_budget,
        )
        .await
    {
        Ok(()) => {}
        Err(error) if is_missing_container_error(&error.to_string()) => return Ok(()),
        Err(error) => {
            tracing::debug!(
                %container_name,
                error = %error,
                "bounded docker stop failed during environment cleanup"
            );
        }
    }
    match backend
        .docker_status_bounded(
            vec!["rm".to_string(), container_name.to_string()],
            deadline.remaining_ms()?,
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if is_missing_container_error(&error.to_string()) => Ok(()),
        Err(error) => backend
            .docker_status_bounded(
                vec!["rm".to_string(), "-f".to_string(), container_name.to_string()],
                deadline.remaining_ms()?,
            )
            .await
            .map_err(|force_error| {
                anyhow::anyhow!(
                    "failed to remove environment container {container_name}: {error}; force removal failed: {force_error}"
                )
            }),
    }
}
