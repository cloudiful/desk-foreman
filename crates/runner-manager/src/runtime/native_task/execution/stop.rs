use anyhow::bail;
use runner_protocol::{
    StopExecutionEnvironmentAck, StopNativeTaskRequest, StopNativeTaskResult,
    execution_environment as protocol,
};
use uuid::Uuid;

use super::{
    identity::{expected_journal, validate_task_identity},
    stop_helpers::{self, NativeStopCandidate},
};
use crate::runtime::{
    DockerRunnerBackend,
    execution_environment::{
        ExecutionEnvironmentProfile,
        deadline::OperationDeadline,
        file_lock::{EnvFileLock, lock_holder_token, release_and_finish},
        inspect::{
            ensure_native_task_labels_match, ensure_native_task_stop_mounts_match,
            inspect_environment_container,
        },
        stop::stop_and_remove_within,
        tombstone::{TombstoneStore, record_stopped},
    },
};

/// Stop one exact native task and prove both runtime and preparation quiescence.
pub(crate) async fn stop_native_task(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: StopNativeTaskRequest,
) -> anyhow::Result<StopNativeTaskResult> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
    let environment_id = environment_id.to_string();
    validate_task_identity(&request.task, generation)?;
    let state_dir = profile.ensure_state_dir()?;
    let deadline = profile.operation_deadline()?;
    let volume_name =
        protocol::derive_state_volume_name(&environment_id).map_err(anyhow::Error::msg)?;
    let store = &backend.native_tasks;
    let uid = store.effective_uid()?;
    if uid == 0 {
        bail!("native task stop refuses to run as root");
    }

    deadline.remaining_ms()?;
    let barrier_store = TombstoneStore::open(state_dir, &environment_id)?;
    let environment_lock = backend.environment_runner_lock(&environment_id);
    let _environment_guard = tokio::time::timeout(deadline.remaining()?, environment_lock.lock())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for the native task stop lock"))?;
    deadline.remaining_ms()?;
    let file_lock = EnvFileLock::acquire(
        &barrier_store.lock_path(),
        lock_holder_token(&backend.manager_id),
    )?;

    let outcome = async {
        deadline.remaining_ms()?;
        let barrier = barrier_store.load()?;
        barrier_store.save(record_stopped(barrier, generation))?;
        backend.service_cancel().cancel_environment(&environment_id);
        deadline.remaining_ms()?;

        // Preparation and start use the same environment/file locks. The task
        // lock additionally covers direct in-process store users and protects
        // the workspace view through the final container inspection.
        let workspace = tokio::time::timeout(
            deadline.remaining()?,
            store.lock_workspace_for_stop(&environment_id, request.task.task_id, uid),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!("timed out waiting for native task preparation to quiesce")
        })??;
        deadline.remaining_ms()?;

        let expected = expected_journal(&request.task, &environment_id, generation);
        let journal = workspace.stop_journal(&expected, uid)?;
        let host_root = backend.config.read().await.native_task_host_root.clone();
        let candidate = stop_helpers::find_candidate(
            backend,
            &request,
            &volume_name,
            host_root.as_deref(),
            &deadline,
        )
        .await?;

        if journal.is_none() && candidate.is_some() {
            bail!("a native task candidate exists without its trusted preparation journal");
        }
        let environment = match candidate {
            None => absent_ack(&request.owner),
            Some(candidate) => {
                if !is_stopped_task_status(&candidate.info.status) {
                    stop_and_remove_within(backend, &candidate.container_name, profile, &deadline)
                        .await?;
                }
                inspect_stopped_candidate(
                    backend,
                    &candidate,
                    &request,
                    &backend.manager_id,
                    &volume_name,
                    host_root.as_deref(),
                    &deadline,
                )
                .await?
            }
        };
        deadline.remaining_ms()?;
        Ok(StopNativeTaskResult {
            task: request.task.clone(),
            environment,
            preparation_quiescent: true,
        })
    }
    .await;
    release_and_finish(file_lock, outcome)
}

async fn inspect_stopped_candidate(
    backend: &DockerRunnerBackend,
    candidate: &NativeStopCandidate,
    request: &StopNativeTaskRequest,
    manager_id: &str,
    state_volume: &str,
    host_root: Option<&std::path::Path>,
    deadline: &OperationDeadline,
) -> anyhow::Result<StopExecutionEnvironmentAck> {
    let Some(info) =
        inspect_environment_container(backend, &candidate.container_name, deadline.remaining_ms()?)
            .await?
    else {
        return Ok(absent_ack(&request.owner));
    };
    let Some(host_root) = host_root else {
        bail!("native task stop requires RUNNER_NATIVE_TASK_HOST_ROOT for a present candidate");
    };
    let (environment_id, _) =
        protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
    let task_source = host_root
        .join(Uuid::parse_str(environment_id)?.to_string())
        .join(request.task.task_id.to_string())
        .join("tree");
    ensure_native_task_labels_match(&info, &request.owner, manager_id, &request.task)?;
    ensure_native_task_stop_mounts_match(&info, state_volume, &task_source)?;
    if !is_stopped_task_status(&info.status) {
        bail!("native task container remains running after stop and removal");
    }
    Ok(StopExecutionEnvironmentAck {
        owner: request.owner.clone(),
        container_name: candidate.container_name.clone(),
        confirmed: true,
        observed_state: info.status,
    })
}

fn absent_ack(owner: &runner_protocol::RunnerOwner) -> StopExecutionEnvironmentAck {
    StopExecutionEnvironmentAck {
        owner: owner.clone(),
        container_name: owner.container_name(),
        confirmed: true,
        observed_state: protocol::EXECUTION_ENVIRONMENT_ABSENT_STATE.to_string(),
    }
}

fn is_stopped_task_status(status: &str) -> bool {
    matches!(status, "exited" | "dead")
}
