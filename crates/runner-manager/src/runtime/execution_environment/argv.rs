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
    build_environment_run_argv_with(
        spec,
        spec.state_workdir,
        &[],
        None,
        None,
        spec.command,
        None,
        &[],
    )
}

/// Build one scoped private-service launch.
///
/// The trusted image owns the entrypoint, so the command is empty; the
/// workspace bind is the only extra mount and the state mount stays the
/// existing durable volume. The state directory is passed as a non-secret
/// environment variable, never as a command or credential argument.
pub fn build_scoped_service_run_argv(
    spec: &EnvironmentRunSpec<'_>,
    image: &str,
    workspace_mount: &str,
    state_directory: &str,
) -> Vec<String> {
    build_environment_run_argv_with(
        spec,
        &spec.config.workdir,
        &[],
        None,
        Some(image),
        &[],
        Some(workspace_mount),
        &[("DF_STATE_DIRECTORY", state_directory.to_string())],
    )
}

/// Fixed root-only ownership script for one derived state volume root.
///
/// The uid and gid arrive as separate positional arguments, so no configured
/// value is ever interpolated into the script text.
const SCOPED_STATE_OWNERSHIP_SCRIPT: &str = "set -eu;chown \"$1:$2\" /state;chmod 0700 /state";

/// Build the one-shot root-owned state alignment for a scoped service.
///
/// This temporary root container can only touch the derived named state
/// volume at `/state`: no workspace bind, no service environment file, no
/// credential, no published port, and no other source. The name is derived
/// from the canonical environment id and generation so one attempt cannot
/// overlap another.
pub fn build_scoped_state_ownership_argv(
    volume_name: &str,
    image: &str,
    uid: u32,
    gid: u32,
    environment_id: &str,
    generation: i64,
) -> Vec<String> {
    let simple: String = environment_id
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    let container_name = format!("df-state-init-{simple}-{generation}");
    let state_mount = format!("type=volume,src={volume_name},dst=/state");
    vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container_name,
        "--network".to_string(),
        "none".to_string(),
        "--read-only".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--cap-add".to_string(),
        "CHOWN".to_string(),
        "--cap-add".to_string(),
        "FOWNER".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--user".to_string(),
        "0:0".to_string(),
        "--entrypoint".to_string(),
        "sh".to_string(),
        "--workdir".to_string(),
        "/".to_string(),
        "--pids-limit".to_string(),
        "32".to_string(),
        "--memory".to_string(),
        "64m".to_string(),
        "--cpus".to_string(),
        "0.1".to_string(),
        "--mount".to_string(),
        state_mount,
        image.to_string(),
        "-c".to_string(),
        SCOPED_STATE_OWNERSHIP_SCRIPT.to_string(),
        "--".to_string(),
        uid.to_string(),
        gid.to_string(),
    ]
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
    build_environment_run_argv_with(
        spec,
        &workspace_directory,
        &labels,
        Some(&task_mount),
        None,
        spec.command,
        None,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn build_environment_run_argv_with(
    spec: &EnvironmentRunSpec<'_>,
    container_workdir: &str,
    extra_labels: &[(&str, String)],
    task_mount: Option<&str>,
    image_override: Option<&str>,
    command: &[String],
    workspace_mount: Option<&str>,
    extra_env: &[(&str, String)],
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
    for (key, value) in extra_env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    if let Some(workspace_mount) = workspace_mount {
        args.push("--mount".to_string());
        args.push(workspace_mount.to_string());
    }
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
    args.push(
        image_override
            .map(str::to_string)
            .unwrap_or_else(|| config.image.clone()),
    );
    args.extend(command.iter().cloned());
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
