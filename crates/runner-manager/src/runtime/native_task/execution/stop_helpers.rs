use std::path::Path;

use anyhow::bail;
use runner_protocol::{StopNativeTaskRequest, execution_environment as protocol};

use crate::{
    config::native_task_paths::validate_native_task_host_root,
    runtime::{
        DockerRunnerBackend,
        execution_environment::{
            LABEL_ENVIRONMENT,
            deadline::OperationDeadline,
            inspect::{
                EnvContainerInfo, ensure_native_task_labels_match,
                ensure_native_task_stop_mounts_match, inspect_environment_container,
            },
        },
    },
};

pub(super) struct NativeStopCandidate {
    pub(super) container_name: String,
    pub(super) info: EnvContainerInfo,
}

pub(super) async fn find_candidate(
    backend: &DockerRunnerBackend,
    request: &StopNativeTaskRequest,
    state_volume: &str,
    host_root: Option<&Path>,
    deadline: &OperationDeadline,
) -> anyhow::Result<Option<NativeStopCandidate>> {
    let (environment_id, generation) =
        protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
    let expected_name = request.owner.container_name();
    let named =
        inspect_environment_container(backend, &expected_name, deadline.remaining_ms()?).await?;
    let named = match named {
        Some(info) => Some(verify_candidate(
            info,
            &expected_name,
            request,
            state_volume,
            host_root,
            &backend.manager_id,
        )?),
        None => None,
    };

    let output = backend
        .docker_output_bounded(
            vec![
                "ps".to_string(),
                "-a".to_string(),
                "--filter".to_string(),
                format!("label={LABEL_ENVIRONMENT}={environment_id}"),
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
        .filter(|name| !name.is_empty() && *name != expected_name)
    {
        let Some(info) =
            inspect_environment_container(backend, name, deadline.remaining_ms()?).await?
        else {
            continue;
        };
        if info.environment_id.as_deref() != Some(environment_id) {
            bail!("an environment-filtered container has mismatched identity labels");
        }
        if info.generation != Some(generation) {
            if protocol::is_running_or_unknown_status(&info.status) {
                bail!("another execution-environment generation may still be running");
            }
            continue;
        }
        let candidate = verify_candidate(
            info,
            name,
            request,
            state_volume,
            host_root,
            &backend.manager_id,
        )?;
        if candidate.container_name != expected_name {
            bail!("native task candidate name does not match its environment owner");
        }
        if named.is_some() {
            bail!("multiple containers are attributed to the native task generation");
        }
        return Ok(Some(candidate));
    }
    Ok(named)
}

fn verify_candidate(
    info: EnvContainerInfo,
    container_name: &str,
    request: &StopNativeTaskRequest,
    state_volume: &str,
    host_root: Option<&Path>,
    manager_id: &str,
) -> anyhow::Result<NativeStopCandidate> {
    ensure_native_task_labels_match(&info, &request.owner, manager_id, &request.task)?;
    let host_root = host_root.ok_or_else(|| {
        anyhow::anyhow!(
            "native task stop requires RUNNER_NATIVE_TASK_HOST_ROOT for a present candidate"
        )
    })?;
    validate_native_task_host_root(host_root)?;
    let (environment_id, _) =
        protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
    let task_source = host_root
        .join(environment_id.to_ascii_lowercase())
        .join(request.task.task_id.to_string());
    let task_source = task_source
        .join("tree")
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("native task host root must be valid UTF-8"))?
        .to_string();
    ensure_native_task_stop_mounts_match(&info, state_volume, Path::new(&task_source))?;
    Ok(NativeStopCandidate {
        container_name: container_name.to_string(),
        info,
    })
}
