//! Command/argv builder tests (issue 749 P1, `#[cfg(test)]`).
//!
//! Proves the hardened run argv carries the trusted command verbatim as
//! the container `CMD`, runs in the explicit state workdir, and keeps the
//! existing hardening, mount, and no-port guarantees.

#[cfg(test)]
mod argv_tests {
    use super::super::argv::{
        EnvironmentRunSpec, ServiceRunSpec, build_environment_run_argv, build_volume_create_argv,
    };
    use crate::config::RunnerManagerConfig;

    fn manager_config() -> RunnerManagerConfig {
        crate::runtime::execution_environment::test_support::test_manager_config("docker")
    }

    fn command() -> Vec<String> {
        vec![
            "/opt/agent/bin/agent".to_string(),
            "serve".to_string(),
            "--mode".to_string(),
            "with spaces".to_string(),
            String::new(),
        ]
    }

    fn spec<'a>(config: &'a RunnerManagerConfig, command: &'a [String]) -> EnvironmentRunSpec<'a> {
        EnvironmentRunSpec {
            container_name: "desk-foreman-runner-execution_environment-abc-0",
            owner_stable_key: "execution_environment:abc:0",
            environment_id: "123e4567-e89b-12d3-a456-426614174000",
            generation: 0,
            manager_id: "test-manager",
            volume_name: "df-env-123e4567e89b12d3a456426614174000",
            state_mount_target: "/state",
            state_workdir: "/state/work",
            command,
            service: None,
            volume_uid: 1000,
            volume_gid: 1000,
            config,
        }
    }

    fn service_spec<'a>(
        config: &'a RunnerManagerConfig,
        command: &'a [String],
    ) -> EnvironmentRunSpec<'a> {
        EnvironmentRunSpec {
            service: Some(ServiceRunSpec {
                network: "df-private",
                credential_env_file: "/var/lib/df/state/123e4567e89b12d3a456426614174000/service/0.env",
            }),
            ..spec(config, command)
        }
    }

    #[test]
    fn environment_argv_is_hardened_without_ports_or_host_mounts() {
        let config = manager_config();
        let command = command();
        let argv = build_environment_run_argv(&spec(&config, &command));
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
    fn configured_command_is_appended_verbatim_after_the_image() {
        let config = manager_config();
        let command = command();
        let argv = build_environment_run_argv(&spec(&config, &command));
        let image_index = argv
            .iter()
            .position(|a| a == "test-image")
            .expect("image present");
        assert_eq!(
            &argv[image_index + 1..],
            command.as_slice(),
            "configured argv must follow the image token-for-token, including spaces and an ordinary empty argument"
        );
    }

    #[test]
    fn working_directory_is_the_explicit_state_workdir() {
        let config = manager_config();
        let command = command();
        let argv = build_environment_run_argv(&spec(&config, &command));
        let workdir_index = argv
            .iter()
            .position(|a| a == "--workdir")
            .expect("explicit workdir");
        assert_eq!(argv[workdir_index + 1], "/state/work");
        assert!(
            !argv.iter().any(|a| a == "-w"),
            "the legacy mount-root workdir flag is gone"
        );
    }

    #[test]
    fn command_adds_no_mounts_ports_or_extra_containers() {
        let config = manager_config();
        let command = command();
        let argv = build_environment_run_argv(&spec(&config, &command));
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == "-v").count(),
            1,
            "the state volume stays the only mount"
        );
        assert_eq!(argv.first().map(String::as_str), Some("run"));
        assert!(
            !argv.iter().any(|a| a == "create" || a == "exec"),
            "run argv must not carry sibling container operations"
        );
    }

    #[test]
    fn without_service_the_network_policy_is_unchanged() {
        let config = manager_config();
        let command = command();
        let argv = build_environment_run_argv(&spec(&config, &command));
        let joined = argv.join(" ");
        assert!(
            joined.contains("--network none"),
            "an unconfigured service keeps the existing network decision, got: {joined}"
        );
        assert!(
            !argv.iter().any(|a| a == "--env-file"),
            "no credential is injected when service support is off"
        );
    }

    #[test]
    fn service_run_uses_the_trusted_network_and_env_file_only() {
        let config = manager_config();
        let command = command();
        let argv = build_environment_run_argv(&service_spec(&config, &command));
        let joined = argv.join(" ");

        assert!(joined.contains("--network df-private"), "got: {joined}");
        assert!(!joined.contains("--network none"), "one network wins");
        assert!(
            joined.contains("--env-file /var/lib/df/state/"),
            "got: {joined}"
        );

        // The credential is addressed by host path only. No token, no
        // published port, and no extra mount could reach the container.
        assert!(!argv.iter().any(|a| a == "-p" || a == "--publish"));
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == "-v").count(),
            1,
            "the credential file is not a mount"
        );
        let network_index = argv
            .iter()
            .position(|a| a == "--network")
            .expect("network flag");
        assert_eq!(argv[network_index + 1], "df-private");
    }

    #[test]
    fn volume_create_never_deletes() {
        let argv = build_volume_create_argv("df-env-abc", "env-abc");
        assert_eq!(argv[0], "volume");
        assert_eq!(argv[1], "create");
        assert!(!argv.iter().any(|a| a == "rm"));
    }

    /// The same hardening guarantees, asserted on the argv a real ensure
    /// emits through the fake CLI rather than on the pure builder.
    #[tokio::test]
    async fn emitted_creation_argv_is_hardened_and_volume_survives() {
        use runner_protocol::EnsureExecutionEnvironmentRequest;

        use super::super::{ensure::ensure_execution_environment, test_support as support};

        let temp = tempfile::tempdir().expect("tempdir");
        let owner = support::env_owner(0);
        let container_name = owner.container_name();
        let log = temp.path().join("create.log");
        std::fs::write(&log, "").expect("seed log");
        // Empty `ps` means no pre-existing container, so the single
        // post-create inspect must report running.
        let running_json = temp.path().join("running.json");
        std::fs::write(
            &running_json,
            support::inspect_json("running", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            running_json.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let ack = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&support::state_dir(temp.path())),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("creation must succeed");
        assert_eq!(ack.container_name, container_name);

        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(logged.contains("--read-only"));
        assert!(logged.contains("--cap-drop"));
        assert!(logged.contains("no-new-privileges:true"));
        assert!(logged.contains(":/state"));
        assert!(logged.contains("--workdir /state/work"));
        assert!(!logged.contains(" --publish"));
        assert!(!logged.contains(" -p "));
        assert!(!logged.contains("/tmp:/workspace"), "no host blanket mount");
        assert!(
            !logged.contains("volume rm"),
            "volume must never be deleted"
        );
    }
}
