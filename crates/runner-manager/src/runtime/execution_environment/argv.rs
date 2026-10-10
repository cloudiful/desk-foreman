//! Hardened `docker run` argv for execution environments (issue 739 P1).
//!
//! Pure builders only: no Docker calls here so fake-CLI tests can assert
//! labels, mounts, and hardening without a daemon. The only mount is the
//! dedicated state volume; task workspaces and the global project root
//! are never mounted. No published ports, no host paths from requests.

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
    pub volume_uid: u32,
    pub volume_gid: u32,
    pub config: &'a RunnerManagerConfig,
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
        "-w".to_string(),
        spec.state_mount_target.to_string(),
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
    if !config.network_enabled {
        args.push("--network".to_string());
        args.push("none".to_string());
    }
    args.push(config.image.clone());
    args.push("sleep".to_string());
    args.push("infinity".to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn manager_config() -> RunnerManagerConfig {
        RunnerManagerConfig {
            control_plane_url: None,
            manager_id: "test-manager".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            auth_token: "test-token".to_string(),
            backend: crate::config::RunnerBackendKind::Docker,
            workspace_root: PathBuf::from("/tmp"),
            host_workspace_root: PathBuf::from("/tmp"),
            image: "test-image".to_string(),
            workdir: "/workspace".to_string(),
            network_enabled: false,
            max_output_bytes: 256,
            max_timeout_ms: 1000,
            max_sessions: 1,
            pids_limit: 64,
            memory_limit: "256m".to_string(),
            cpu_limit: "1".to_string(),
            idle_ttl: Duration::from_secs(1),
            docker_cli: "docker".to_string(),
            docker_host: None,
            runtime_class: None,
        }
    }

    #[test]
    fn environment_argv_is_hardened_without_ports_or_host_mounts() {
        let config = manager_config();
        let spec = EnvironmentRunSpec {
            container_name: "desk-foreman-runner-execution_environment-abc-0",
            owner_stable_key: "execution_environment:abc:0",
            environment_id: "123e4567-e89b-12d3-a456-426614174000",
            generation: 0,
            manager_id: "test-manager",
            volume_name: "df-env-123e4567e89b12d3a456426614174000",
            state_mount_target: "/state",
            volume_uid: 1000,
            volume_gid: 1000,
            config: &config,
        };
        let argv = build_environment_run_argv(&spec);
        let joined = argv.join(" ");
        assert!(argv.contains(&"--read-only".to_string()));
        assert!(argv.contains(&"--cap-drop".to_string()));
        assert!(argv.contains(&"no-new-privileges:true".to_string()));
        assert!(joined.contains("df-env-123e4567e89b12d3a456426614174000:/state"));
        assert!(joined.contains("desk-foreman.execution-environment=123e4567"));
        assert!(joined.contains("desk-foreman.generation=0"));
        assert!(!argv.iter().any(|a| a == "-p" || a == "--publish"));
        assert!(!joined.contains("/tmp:/workspace"), "no host blanket mount");
        assert!(!joined.contains("WORKSPACE_ROOT"), "no workspace env leak");
        assert!(joined.contains("--user 1000:1000"));
        assert!(joined.contains("--network none"));
    }

    #[test]
    fn volume_create_never_deletes() {
        let argv = build_volume_create_argv("df-env-abc", "env-abc");
        assert_eq!(argv[0], "volume");
        assert_eq!(argv[1], "create");
        assert!(!argv.iter().any(|a| a == "rm"));
    }
}
