//! Hardened `docker run` argv for execution environments (issue 739 P1,
//! trusted process command in 749 P1).
//!
//! Pure builders only: no Docker calls here so fake-CLI tests can assert
//! labels, mounts, and hardening without a daemon. The only mount is the
//! dedicated state volume; task workspaces and the global project root
//! are never mounted. No published ports, no host paths from requests.
//!
//! The configured trusted command is appended verbatim as the container
//! `CMD`, after the image. The image's own `ENTRYPOINT` therefore still
//! decides what actually runs, so the configured image must be one whose
//! entrypoint accepts a command of this shape; no entrypoint override is
//! invented here.

use super::{LABEL_ENVIRONMENT, LABEL_GENERATION, LABEL_MANAGED, LABEL_MANAGER, LABEL_OWNER};
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

/// Build the hardened `docker run` argv. Never emits `-p`/`--publish`,
/// host workspace binds, or request-supplied images.
pub fn build_environment_run_argv(spec: &EnvironmentRunSpec<'_>) -> Vec<String> {
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
        "-e".to_string(),
        format!("DF_EXECUTION_ENVIRONMENT={}", spec.environment_id),
        "-e".to_string(),
        format!("DF_GENERATION={}", spec.generation),
        "-v".to_string(),
        format!("{}:{}", spec.volume_name, spec.state_mount_target),
        "--workdir".to_string(),
        spec.state_workdir.to_string(),
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
    ];
    if let Some(runtime_class) = &config.runtime_class {
        args.push("--runtime".to_string());
        args.push(runtime_class.clone());
    }
    if let Some(service) = spec.service {
        // Explicit, trusted network. The manager deployment must already
        // join this network; nothing here reaches out to create or attach
        // to one, and no port is ever published for the agent.
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
