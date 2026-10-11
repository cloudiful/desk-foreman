use anyhow::bail;
use runner_protocol::{StartNativeTaskRequest, StartNativeTaskResult};

use super::super::{journal, store::LockedNativeTaskWorkspace};
use super::{candidates, identity::start_result};
use crate::{
    config::RunnerManagerConfig,
    runtime::{
        DockerRunnerBackend,
        execution_environment::{
            argv,
            deadline::OperationDeadline,
            inspect::inspect_environment_container,
            profile::EnsureProcessProfile,
            service::ServiceProfileConfig,
            state_probe::{confirm_state_writable, confirm_task_workspace_writable},
        },
    },
};

const START_FAILURE: &str = "native task start could not establish the requested container";

#[allow(clippy::too_many_arguments)]
pub(super) async fn start_prepared_task(
    backend: &DockerRunnerBackend,
    request: &StartNativeTaskRequest,
    config: &RunnerManagerConfig,
    process: &EnsureProcessProfile<'_>,
    service: Option<&ServiceProfileConfig>,
    credential_env_file: Option<&str>,
    volume_name: &str,
    state_mount_target: &str,
    task_host_tree: &str,
    container_name: &str,
    starting: &journal::Journal,
    workspace: &LockedNativeTaskWorkspace,
    uid: u32,
    deadline: &OperationDeadline,
) -> anyhow::Result<StartNativeTaskResult> {
    ensure_docker_success(
        "create the task state volume",
        backend
            .docker_output_owned(
                argv::build_volume_create_argv(volume_name, &starting.environment_id),
                Some(deadline.remaining_ms()?),
                None,
            )
            .await?,
    )?;

    deadline.remaining_ms()?;
    let service_run = match (service, credential_env_file) {
        (Some(service), Some(env_file)) => Some(argv::ServiceRunSpec {
            network: &service.network,
            credential_env_file: env_file,
        }),
        _ => None,
    };
    let owner_stable_key = request.owner.stable_key();
    let spec = argv::EnvironmentRunSpec {
        container_name,
        owner_stable_key: &owner_stable_key,
        environment_id: &starting.environment_id,
        generation: starting.generation,
        manager_id: &config.manager_id,
        volume_name,
        state_mount_target,
        state_workdir: process.state_workdir,
        command: process.command,
        service: service_run,
        volume_uid: process.uid,
        volume_gid: process.gid,
        config,
    };
    ensure_docker_success(
        "create the native task container",
        backend
            .docker_output_owned(
                argv::build_native_task_run_argv(&spec, &request.task, task_host_tree),
                Some(deadline.remaining_ms()?),
                None,
            )
            .await?,
    )?;

    let Some(info) =
        inspect_environment_container(backend, container_name, deadline.remaining_ms()?).await?
    else {
        bail!("{START_FAILURE}");
    };
    candidates::verify_native_candidate(
        &info,
        container_name,
        request,
        &config.manager_id,
        &config.image,
        volume_name,
        state_mount_target,
        task_host_tree,
        process.uid,
        process.gid,
    )?;
    if info.status != "running" {
        bail!("{START_FAILURE}");
    }

    confirm_state_writable(backend, container_name, process, deadline).await?;
    confirm_task_workspace_writable(backend, container_name, process, deadline).await?;
    deadline.remaining_ms()?;

    let mut running = starting.clone();
    running.phase = journal::JournalPhase::Running;
    workspace.mark_running(&running, uid)?;
    Ok(start_result(request, container_name.to_string()))
}

fn ensure_docker_success(
    action: &str,
    output: runner_protocol::CommandOutput,
) -> anyhow::Result<()> {
    if output.timed_out {
        bail!("native task manager timed out while trying to {action}");
    }
    if output.exit_code != Some(0) {
        bail!("native task manager failed to {action}");
    }
    Ok(())
}
