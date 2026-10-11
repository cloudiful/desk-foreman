//! Shared environment-container inspection and owner guard (issue 740 P1).
//!
//! `ensure_owner_matches` is the single owner/environment/generation
//! verification used by ensure reuse and post-create/post-start checks and
//! by stop preflight and final inspection. Inspection is bounded by the
//! caller's remaining operation budget and fails closed: timeouts and
//! transport/parse/permission failures never become absence, and absence
//! is only ever the known `No such container/object` error.

use std::path::{Component, Path};

use runner_protocol::{NativeTaskRuntimeIdentity, RunnerOwner, execution_environment as protocol};

use super::{
    LABEL_ENVIRONMENT, LABEL_GENERATION, LABEL_MANAGED, LABEL_MANAGER, LABEL_NATIVE_BINDING_ID,
    LABEL_NATIVE_PREPARATION_GENERATION, LABEL_NATIVE_SEED_SHA256, LABEL_NATIVE_TASK_ID,
    LABEL_OWNER, NATIVE_TASK_WORKDIR,
};
use crate::runtime::docker::DockerRunnerBackend;
use crate::runtime::docker_command::is_missing_container_error;

#[derive(Clone, Debug)]
pub(crate) struct EnvContainerInfo {
    pub(crate) status: String,
    pub(crate) owner_key: Option<String>,
    pub(crate) environment_id: Option<String>,
    pub(crate) generation: Option<i64>,
    pub(crate) image: Option<String>,
    pub(crate) labels: std::collections::HashMap<String, String>,
    pub(crate) user: Option<String>,
    pub(crate) working_dir: Option<String>,
    pub(crate) privileged: Option<bool>,
    pub(crate) publish_all_ports: Option<bool>,
    pub(crate) port_bindings_empty: bool,
    pub(crate) mounts: Option<Vec<ContainerMount>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerMount {
    pub(crate) kind: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) destination: Option<String>,
    pub(crate) read_write: Option<bool>,
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

/// Require the exact manager, owner and native-task labels before a task
/// candidate is reused or stopped.
pub(crate) fn ensure_native_task_labels_match(
    info: &EnvContainerInfo,
    owner: &RunnerOwner,
    manager_id: &str,
    task: &NativeTaskRuntimeIdentity,
) -> anyhow::Result<()> {
    ensure_owner_matches(info, owner)?;
    let expected_labels = [
        (LABEL_MANAGER, manager_id.to_string()),
        (LABEL_MANAGED, "true".to_string()),
        (LABEL_NATIVE_TASK_ID, task.task_id.to_string()),
        (
            LABEL_NATIVE_BINDING_ID,
            task.workspace_binding_id.to_string(),
        ),
        (LABEL_NATIVE_SEED_SHA256, task.manifest_sha256.clone()),
        (
            LABEL_NATIVE_PREPARATION_GENERATION,
            task.preparation_generation.to_string(),
        ),
    ];
    if expected_labels
        .iter()
        .any(|(key, expected)| info.labels.get(*key) != Some(expected))
    {
        anyhow::bail!("native task container labels do not match the stop or start identity");
    }
    Ok(())
}

/// Require the native identity, exact launch mounts and non-root process
/// configuration before acknowledging or reusing a start.
pub(crate) fn ensure_native_task_container_matches(
    info: &EnvContainerInfo,
    owner: &RunnerOwner,
    manager_id: &str,
    expected_image: &str,
    task: &NativeTaskRuntimeIdentity,
    state_volume: &str,
    state_mount_target: &str,
    task_source: &Path,
    uid: u32,
    gid: u32,
) -> anyhow::Result<()> {
    ensure_native_task_labels_match(info, owner, manager_id, task)?;
    if info.image.as_deref() != Some(expected_image)
        || info.user.as_deref() != Some(format!("{uid}:{gid}").as_str())
        || info.working_dir.as_deref() != Some(NATIVE_TASK_WORKDIR)
        || info.privileged != Some(false)
        || info.publish_all_ports != Some(false)
        || !info.port_bindings_empty
    {
        anyhow::bail!("native task container identity or process configuration does not match");
    }

    let mounts = info
        .mounts
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("native task container mount metadata is unavailable"))?;
    let task_source = task_source.to_string_lossy();
    let has_state = mounts.iter().any(|mount| {
        mount.kind.as_deref() == Some("volume")
            && mount.name.as_deref() == Some(state_volume)
            && mount.destination.as_deref() == Some(state_mount_target)
            && mount.read_write == Some(true)
    });
    let has_task = mounts.iter().any(|mount| {
        mount.kind.as_deref() == Some("bind")
            && mount.source.as_deref() == Some(task_source.as_ref())
            && mount.destination.as_deref() == Some(NATIVE_TASK_WORKDIR)
            && mount.read_write == Some(true)
    });
    if mounts.len() != 2 || !has_state || !has_task {
        anyhow::bail!("native task container mounts do not match its private task and state roots");
    }
    Ok(())
}

/// Validate only the two durable mounts required to identify a native stop
/// candidate, independent of the current launch profile.
pub(crate) fn ensure_native_task_stop_mounts_match(
    info: &EnvContainerInfo,
    state_volume: &str,
    task_source: &Path,
) -> anyhow::Result<()> {
    let mounts = info
        .mounts
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("native task container mount metadata is unavailable"))?;
    let task_source = task_source.to_string_lossy();
    let has_state = mounts.iter().any(|mount| {
        mount.kind.as_deref() == Some("volume")
            && mount.name.as_deref() == Some(state_volume)
            && mount.destination.as_deref().is_some_and(|destination| {
                let destination = Path::new(destination);
                destination.is_absolute()
                    && !destination
                        .components()
                        .any(|component| component == Component::ParentDir)
                    && !destination.starts_with(Path::new(NATIVE_TASK_WORKDIR))
                    && !Path::new(NATIVE_TASK_WORKDIR).starts_with(destination)
            })
            && mount.read_write == Some(true)
    });
    let has_task = mounts.iter().any(|mount| {
        mount.kind.as_deref() == Some("bind")
            && mount.source.as_deref() == Some(task_source.as_ref())
            && mount.destination.as_deref() == Some(NATIVE_TASK_WORKDIR)
            && mount.read_write == Some(true)
    });
    if mounts.len() != 2 || !has_state || !has_task {
        anyhow::bail!("native task stop candidate mounts do not match its task and state volumes");
    }
    Ok(())
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
    let config = value.get("Config");
    let labels = config.and_then(|item| item.get("Labels"));
    let get_label = |key: &str| {
        labels
            .and_then(|item| item.get(key))
            .and_then(|item| item.as_str())
            .map(str::to_string)
    };
    let labels = labels
        .and_then(serde_json::Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let generation = get_label(LABEL_GENERATION).and_then(|raw| raw.parse::<i64>().ok());
    let host_config = value.get("HostConfig");
    let port_bindings_empty = host_config
        .and_then(|item| item.get("PortBindings"))
        .is_some_and(|bindings| {
            bindings.is_null() || bindings.as_object().is_some_and(serde_json::Map::is_empty)
        });
    let mounts = value
        .get("Mounts")
        .and_then(serde_json::Value::as_array)
        .map(|mounts| {
            mounts
                .iter()
                .map(|mount| ContainerMount {
                    kind: mount
                        .get("Type")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    name: mount
                        .get("Name")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    source: mount
                        .get("Source")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    destination: mount
                        .get("Destination")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    read_write: mount.get("RW").and_then(|value| value.as_bool()),
                })
                .collect()
        });
    Ok(Some(EnvContainerInfo {
        status,
        owner_key: get_label(LABEL_OWNER),
        environment_id: get_label(LABEL_ENVIRONMENT),
        generation,
        image: config
            .and_then(|item| item.get("Image"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        labels,
        user: config
            .and_then(|item| item.get("User"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        working_dir: config
            .and_then(|item| item.get("WorkingDir"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        privileged: host_config
            .and_then(|item| item.get("Privileged"))
            .and_then(|value| value.as_bool()),
        publish_all_ports: host_config
            .and_then(|item| item.get("PublishAllPorts"))
            .and_then(|value| value.as_bool()),
        port_bindings_empty,
        mounts,
    }))
}
