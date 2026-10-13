use anyhow::bail;
use runner_protocol::{
    StartNativeTaskRequest, StartNativeTaskResult, execution_environment as protocol,
};
use uuid::Uuid;

use super::super::journal;
use super::{
    candidates,
    identity::{expected_journal, start_result, validate_task_identity},
};
use crate::{
    config::native_task_paths::validate_native_task_host_root,
    runtime::{
        DockerRunnerBackend,
        execution_environment::{
            ExecutionEnvironmentProfile,
            file_lock::{EnvFileLock, lock_holder_token, release_and_finish},
            service::ServiceCredentialStore,
            state_probe::{confirm_state_writable, confirm_task_workspace_writable},
            tombstone::{TombstoneStore, advance_high_water, ensure_generation_admissible},
        },
    },
};

/// Start one prepared task on its original execution-environment manager.
pub(crate) async fn start_native_task(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: StartNativeTaskRequest,
) -> anyhow::Result<StartNativeTaskResult> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
    validate_task_identity(&request.task, generation)?;

    profile.ensure_enabled()?;
    let process = profile.ensure_process_profile()?;
    let service = profile.ensure_service_profile()?;
    let state_dir = profile.ensure_state_dir()?;
    let deadline = profile.operation_deadline()?;
    let volume_name =
        protocol::derive_state_volume_name(environment_id).map_err(anyhow::Error::msg)?;
    let state_mount_target = profile.state_mount_target.clone();
    let credentials = ServiceCredentialStore::new(state_dir);
    let config = backend.config.read().await.clone();
    let host_root = config.native_task_host_root.as_deref().ok_or_else(|| {
        anyhow::anyhow!("native task start requires RUNNER_NATIVE_TASK_HOST_ROOT")
    })?;
    validate_native_task_host_root(host_root)?;

    let store = &backend.native_tasks;
    let uid = store.effective_uid()?;
    if uid == 0 || process.uid != uid {
        bail!("native task profile user must match the non-root manager uid");
    }
    let environment_uuid = Uuid::parse_str(environment_id)
        .map_err(|_| anyhow::anyhow!("native task environment identity is invalid"))?;
    let task_host_tree = host_root
        .join(environment_uuid.to_string())
        .join(request.task.task_id.to_string())
        .join("tree");
    let task_host_tree = task_host_tree
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("native task host root must be valid UTF-8"))?
        .to_string();

    deadline.remaining_ms()?;
    let barrier_store = TombstoneStore::open(state_dir, environment_id)?;
    let environment_lock = backend.environment_runner_lock(environment_id);
    let _environment_guard = tokio::time::timeout(deadline.remaining()?, environment_lock.lock())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for the environment start lock"))?;
    deadline.remaining_ms()?;
    let file_lock = EnvFileLock::acquire(
        &barrier_store.lock_path(),
        lock_holder_token(&backend.manager_id),
    )?;

    let outcome = async {
        let barrier = barrier_store.load()?;
        ensure_generation_admissible(&barrier, generation)?;
        barrier_store.save(advance_high_water(barrier, generation))?;
        deadline.remaining_ms()?;

        deadline.remaining_ms()?;
        let workspace = tokio::time::timeout(
            deadline.remaining()?,
            store.lock_workspace(environment_id, request.task.task_id, uid),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for the native task lock"))??;
        deadline.remaining_ms()?;

        let expected = expected_journal(&request.task, environment_id, generation);
        let existing = workspace.verified_journal(&expected, uid)?;
        match existing.phase {
            journal::JournalPhase::Prepared => {
                deadline.remaining_ms()?;
                workspace.verify_prepared_tree(&existing, uid)?;
            }
            journal::JournalPhase::Running => {
                workspace.validate_running_tree(uid)?;
            }
            journal::JournalPhase::Starting => {
                bail!("an interrupted native task start requires explicit stop reconciliation");
            }
            journal::JournalPhase::Preparing => {
                bail!("an interrupted native task preparation requires explicit reconciliation");
            }
        }

        let credential_env_file = if existing.phase == journal::JournalPhase::Prepared {
            match service {
                Some(service) => {
                    deadline.remaining_ms()?;
                    Some(
                        credentials
                            .obtain(environment_id, generation, &service.auth_env_name)?
                            .env_file()
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
                None => None,
            }
        } else {
            None
        };

        let container_name = request.owner.container_name();
        let candidate = candidates::find_native_candidate(
            backend,
            &request,
            &container_name,
            &config.manager_id,
            &config.image,
            &volume_name,
            &state_mount_target,
            &task_host_tree,
            process.uid,
            process.gid,
            &deadline,
        )
        .await?;
        match (existing.phase, candidate) {
            (journal::JournalPhase::Running, Some(candidate)) => {
                if candidate.info.status != "running" {
                    bail!("the journaled native task container is not running");
                }
                if let Some(service) = service {
                    deadline.remaining_ms()?;
                    credentials.existing(environment_id, generation, &service.auth_env_name)?;
                }
                confirm_state_writable(backend, &container_name, &process, &deadline).await?;
                confirm_task_workspace_writable(
                    backend,
                    &container_name,
                    request.task.workspace_binding_id,
                    &process,
                    &deadline,
                )
                .await?;
                deadline.remaining_ms()?;
                Ok(start_result(&request, container_name))
            }
            (journal::JournalPhase::Running, None) => {
                bail!("the running native task container is missing; refusing to recreate it")
            }
            (journal::JournalPhase::Prepared, Some(_)) => {
                bail!("a native task container exists without a running journal; refusing adoption")
            }
            (journal::JournalPhase::Prepared, None) => {
                deadline.remaining_ms()?;
                let mut starting = existing.clone();
                starting.phase = journal::JournalPhase::Starting;
                workspace.mark_starting(&starting, uid)?;
                super::launch::start_prepared_task(
                    backend,
                    &request,
                    &config,
                    &process,
                    service,
                    credential_env_file.as_deref(),
                    &volume_name,
                    &state_mount_target,
                    &task_host_tree,
                    &container_name,
                    &starting,
                    &workspace,
                    uid,
                    &deadline,
                )
                .await
            }
            (journal::JournalPhase::Starting | journal::JournalPhase::Preparing, _) => {
                unreachable!("incomplete journals were rejected before container inspection")
            }
        }
    }
    .await;
    release_and_finish(file_lock, outcome)
}
