//! Hardened `docker run` argv for execution environments (issue 739 P1,
//! trusted process command in 749 P1).
//!
//! Pure builders only: no Docker calls here so fake-CLI tests can assert
//! labels, mounts, and hardening without a daemon. Legacy environment launch
//! keeps its state-volume-only mount; native launch adds one server-derived
//! task bind at the binding-scoped directory shared with the reuse/stop mount
//! guards. No published ports or request-supplied host paths are accepted.
//!
//! The configured trusted command is appended verbatim as the container
//! `CMD`, after the image. The image's own `ENTRYPOINT` therefore still
//! decides what actually runs, so the configured image must be one whose
//! entrypoint accepts a command of this shape; no entrypoint override is
//! invented here.

use runner_protocol::{NativeTaskRuntimeIdentity, native_task_workspace_directory};

use super::{
    LABEL_ENVIRONMENT, LABEL_GENERATION, LABEL_MANAGED, LABEL_MANAGER, LABEL_NATIVE_BINDING_ID,
    LABEL_NATIVE_PREPARATION_GENERATION, LABEL_NATIVE_SEED_SHA256, LABEL_NATIVE_TASK_ID,
    LABEL_OWNER,
};
use crate::config::RunnerManagerConfig;

/// Inputs for one environment container creation. Image, limits, and
/// network come solely from trusted manager config; the request only
/// supplies identity.
pub struct EnvironmentRunSpec<'a> {
    pub container_name: &'a str,
    pub owner_stable_key: &'a str,
    pub environment_id: &'a str,
    pub generation: i64,
    pub manager_id: &'a str,
    pub volume_name: &'a str,
    pub state_mount_target: &'a str,
    /// Validated absolute working directory inside the state mount.
    pub state_workdir: &'a str,
    /// Validated trusted command argv, appended verbatim after the image.
    pub command: &'a [String],
    /// Optional private-service launch: the trusted Docker network to join
    /// and the host credential file to inject. `None` leaves the container
    /// network policy exactly as the manager config already decided.
    pub service: Option<ServiceRunSpec<'a>>,
    pub volume_uid: u32,
    pub volume_gid: u32,
    pub config: &'a RunnerManagerConfig,
}

/// Private-service inputs for one container creation. Carries the trusted
/// network name and a host-side credential *path* — never the credential
/// value, which must not appear in an argv where any local process could
/// read it.
#[derive(Clone, Copy, Debug)]
pub struct ServiceRunSpec<'a> {
    /// Docker network the container joins explicitly.
    pub network: &'a str,
    /// Host path passed to `--env-file`.
    pub credential_env_file: &'a str,
}

/// Build the hardened legacy environment argv, preserving its state-only mount.
pub fn build_environment_run_argv(spec: &EnvironmentRunSpec<'_>) -> Vec<String> {
    build_environment_run_argv_with(spec, spec.state_workdir, &[], None)
}

/// Build one native-task launch with a typed identity and derived task bind.
///
/// The bind destination and the container working directory are the same
/// binding-scoped directory from the shared protocol helper, while the host
/// source stays the authorized checkout this manager already materialized.
/// Nothing here is caller-provided.
pub fn build_native_task_run_argv(
    spec: &EnvironmentRunSpec<'_>,
    task: &NativeTaskRuntimeIdentity,
    task_tree_source: &str,
) -> Vec<String> {
    let labels = [
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
    let workspace_directory = native_task_workspace_directory(task.workspace_binding_id);
    let task_mount = format!("type=bind,source={task_tree_source},target={workspace_directory}");
    build_environment_run_argv_with(spec, &workspace_directory, &labels, Some(&task_mount))
}

fn build_environment_run_argv_with(
    spec: &EnvironmentRunSpec<'_>,
    container_workdir: &str,
    extra_labels: &[(&str, String)],
    task_mount: Option<&str>,
) -> Vec<String> {
    let config = spec.config;
    let mut args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        spec.container_name.to_string(),
        "--user".to_string(),
        format!("{}:{}", spec.volume_uid, spec.volume_gid),
        "--label".to_string(),
        format!("{LABEL_OWNER}={}", spec.owner_stable_key),
        "--label".to_string(),
        format!("{LABEL_MANAGED}=true"),
        "--label".to_string(),
        format!("{LABEL_MANAGER}={}", spec.manager_id),
        "--label".to_string(),
        format!("{LABEL_ENVIRONMENT}={}", spec.environment_id),
        "--label".to_string(),
        format!("{LABEL_GENERATION}={}", spec.generation),
    ];
    for (key, value) in extra_labels {
        args.push("--label".to_string());
        args.push(format!("{key}={value}"));
    }
    args.extend([
        "-e".to_string(),
        format!("DF_EXECUTION_ENVIRONMENT={}", spec.environment_id),
        "-e".to_string(),
        format!("DF_GENERATION={}", spec.generation),
        "-v".to_string(),
        format!("{}:{}", spec.volume_name, spec.state_mount_target),
    ]);
    if let Some(task_mount) = task_mount {
        args.push("--mount".to_string());
        args.push(task_mount.to_string());
    }
    args.extend([
        "--workdir".to_string(),
        container_workdir.to_string(),
        "--read-only".to_string(),
        "--tmpfs".to_string(),
        "/tmp:rw,nosuid,size=256m".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges:true".to_string(),
        "--pids-limit".to_string(),
        config.pids_limit.to_string(),
        "--memory".to_string(),
        config.memory_limit.clone(),
        "--cpus".to_string(),
        config.cpu_limit.clone(),
    ]);
    if let Some(runtime_class) = &config.runtime_class {
        args.push("--runtime".to_string());
        args.push(runtime_class.clone());
    }
    if let Some(service) = spec.service {
        args.push("--network".to_string());
        args.push(service.network.to_string());
        args.push("--env-file".to_string());
        args.push(service.credential_env_file.to_string());
    } else if !config.network_enabled {
        args.push("--network".to_string());
        args.push("none".to_string());
    }
    args.push(config.image.clone());
    args.extend(spec.command.iter().cloned());
    args
}

/// Build `docker volume create` argv for the durable state volume.
/// Invoked only by runtime code/tests through the fake harness, never as
/// a live CLI during development.
pub fn build_volume_create_argv(volume_name: &str, environment_id: &str) -> Vec<String> {
    vec![
        "volume".to_string(),
        "create".to_string(),
        "--label".to_string(),
        format!("{LABEL_MANAGED}=true"),
        "--label".to_string(),
        format!("{LABEL_ENVIRONMENT}={environment_id}"),
        volume_name.to_string(),
    ]
}
