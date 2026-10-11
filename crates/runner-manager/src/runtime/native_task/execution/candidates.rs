use std::path::Path;

use anyhow::bail;
use runner_protocol::{StartNativeTaskRequest, execution_environment as protocol};

use crate::runtime::execution_environment::deadline::OperationDeadline;
use crate::runtime::{
    DockerRunnerBackend,
    execution_environment::{
        LABEL_ENVIRONMENT,
        inspect::{
            ContainerMount, EnvContainerInfo, ensure_native_task_container_matches,
            inspect_environment_container,
        },
    },
};

pub(super) struct NativeCandidate {
    pub(super) info: EnvContainerInfo,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn find_native_candidate(
    backend: &DockerRunnerBackend,
    request: &StartNativeTaskRequest,
    expected_name: &str,
    manager_id: &str,
    expected_image: &str,
    state_volume: &str,
    state_mount_target: &str,
    task_source: &str,
    uid: u32,
    gid: u32,
    deadline: &OperationDeadline,
) -> anyhow::Result<Option<NativeCandidate>> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
    let named =
        inspect_environment_container(backend, expected_name, deadline.remaining_ms()?).await?;
    if let Some(info) = &named {
        verify_native_candidate(
            info,
            expected_name,
            request,
            manager_id,
            expected_image,
            state_volume,
            state_mount_target,
            task_source,
            uid,
            gid,
        )?;
    }

    let filter = format!("label={LABEL_ENVIRONMENT}={environment_id}");
    let output = backend
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
    if output.timed_out || output.exit_code != Some(0) {
        bail!("native task container survey failed within the operation deadline");
    }
    for name in output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if name == expected_name {
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
        if info.generation == Some(generation) {
            bail!("another container is already attributed to this native task generation");
        }
        if protocol::is_running_or_unknown_status(&info.status) {
            bail!("another execution-environment generation may still be running");
        }
    }
    Ok(named.map(|info| NativeCandidate { info }))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn verify_native_candidate(
    info: &EnvContainerInfo,
    container_name: &str,
    request: &StartNativeTaskRequest,
    manager_id: &str,
    expected_image: &str,
    state_volume: &str,
    state_mount_target: &str,
    task_source: &str,
    uid: u32,
    gid: u32,
) -> anyhow::Result<()> {
    ensure_native_task_container_matches(
        info,
        &request.owner,
        manager_id,
        expected_image,
        &request.task,
        state_volume,
        state_mount_target,
        Path::new(task_source),
        uid,
        gid,
    )?;
    let mounts = info.mounts.as_deref().unwrap_or_default();
    if mounts.len() != 2 || !mounts.iter().all(native_mount_is_bounded) {
        bail!("native task container has an untrusted mount configuration");
    }
    if container_name != request.owner.container_name() {
        bail!("native task container name does not match its environment owner");
    }
    Ok(())
}

fn native_mount_is_bounded(mount: &ContainerMount) -> bool {
    matches!(mount.kind.as_deref(), Some("volume" | "bind")) && mount.read_write == Some(true)
}
