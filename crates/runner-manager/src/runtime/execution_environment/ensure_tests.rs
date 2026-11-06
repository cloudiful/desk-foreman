//! Ensure-path fake-Docker tests (issue 739 P1, `#[cfg(test)]`).
//!
//! Proves disabled fail-closed, same-generation idempotence, old/unknown
//! refusal without deletion, malformed rejection, stable volumes,
//! hardening argv, env-level serialization, legacy stability, and
//! unsupported backends. No live daemon.

#[cfg(test)]
mod ensure_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use runner_protocol::{
        EnsureExecutionEnvironmentRequest, ExecutionEnvironmentWorkspaceScope, RunnerOwner,
        StopExecutionEnvironmentRequest,
    };

    use super::super::{
        ExecutionEnvironmentProfile,
        ensure::ensure_execution_environment,
        profile::{
            command::{TrustedCommand, parse_trusted_command},
            workdir::{TrustedStateWorkdir, parse_state_workdir},
        },
        stop::stop_execution_environment,
        test_support as support,
    };
    use crate::runtime::RunnerBackend;

    #[tokio::test]
    async fn disabled_profile_rejects_ensure_without_docker_calls() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("disabled.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let owner = support::env_owner(0);
        let result = ensure_execution_environment(
            &backend,
            &ExecutionEnvironmentProfile::default(),
            EnsureExecutionEnvironmentRequest {
                owner,
                workspace_scope: None,
            },
        )
        .await;
        assert!(result.is_err(), "disabled profile must fail closed");
        assert!(
            result.unwrap_err().to_string().contains("disabled"),
            "error must name the disabled profile"
        );
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            logged.is_empty(),
            "disabled must not touch docker, got: {logged}"
        );
    }

    #[tokio::test]
    async fn scoped_ensure_creates_and_reuses_one_validated_private_service() {
        use std::os::unix::fs::MetadataExt;

        use runner_protocol::execution_environment::derive_state_volume_name;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let local_root = workspace.join("scope");
        let state = temp.path().join("state");
        std::fs::create_dir_all(&local_root).expect("workspace scope");
        std::fs::create_dir_all(&state).expect("state dir");
        let owner = support::env_owner(0);
        let container = owner.container_name();
        let owner_key = owner.stable_key();
        let metadata = std::fs::metadata(&local_root).expect("local root metadata");
        let (uid, gid) = (metadata.uid(), metadata.gid());
        let volume = derive_state_volume_name(support::ENV_ID).expect("volume");
        let image = format!(
            "registry.example.com/team/service@sha256:{}",
            "a".repeat(64)
        );
        let inspect = serde_json::json!({
            "Name": format!("/{container}"),
            "Id": "scoped-helper-id",
            "State": {"Status": "running"},
            "Config": {
                "Image": image,
                "User": format!("{uid}:{gid}"),
                "WorkingDir": "/workspace",
                "Labels": {
                    "desk-foreman.owner": owner_key,
                    "desk-foreman.manager": "test-manager",
                    "desk-foreman.managed": "true",
                    "desk-foreman.execution-environment": support::ENV_ID,
                    "desk-foreman.generation": "0"
                }
            },
            "HostConfig": {
                "Privileged": false,
                "PublishAllPorts": false,
                "PortBindings": {}
            },
            "Mounts": [
                {
                    "Type": "volume",
                    "Name": volume,
                    "Source": format!("/var/lib/docker/volumes/{volume}/_data"),
                    "Destination": "/state",
                    "RW": true
                },
                {
                    "Type": "bind",
                    "Source": local_root.to_string_lossy(),
                    "Destination": "/workspace",
                    "RW": true
                },
                {"Type": "tmpfs", "Destination": "/tmp", "RW": true}
            ]
        })
        .to_string();

        let run = |mode: &str| {
            let log = temp.path().join("scoped.log");
            std::fs::write(&log, "").expect("seed log");
            let inspect_file = temp.path().join("scoped-inspect.json");
            std::fs::write(&inspect_file, &inspect).expect("inspect json");
            let script = format!(
                "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  if [ \"{}\" = \"reuse\" ]; then echo \"{}\"; fi\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
                log.display(),
                mode,
                container,
                inspect_file.display()
            );
            let fake = support::write_executable(temp.path(), &format!("docker-{mode}"), &script);
            let backend = support::backend_with_cli(&fake.to_string_lossy());
            (backend, log)
        };

        let profile = ExecutionEnvironmentProfile {
            enabled: true,
            stop_timeout: Duration::from_millis(500),
            operation_deadline: Duration::from_secs(5),
            volume_uid: Some(uid),
            volume_gid: Some(gid),
            state_workdir: TrustedStateWorkdir::Configured("/state/work".to_string()),
            service: super::super::service::ServiceProfile::Configured(
                super::super::service::ServiceProfileConfig {
                    network: "df-private".to_string(),
                    port: 8081,
                    auth_username: "agent".to_string(),
                    auth_env_name: "DF_SERVICE_TOKEN".to_string(),
                },
            ),
            state_dir: Some(state.clone()),
            ..ExecutionEnvironmentProfile::default()
        };
        unsafe {
            std::env::set_var(
                crate::runtime::workspace_service::WORKSPACE_SERVICE_IMAGE_ENV,
                &image,
            );
        }
        let request = || EnsureExecutionEnvironmentRequest {
            owner: owner.clone(),
            workspace_scope: Some(ExecutionEnvironmentWorkspaceScope {
                workspace_binding_id: 5,
                workspace_relative_path: "scope".to_string(),
            }),
        };

        let (backend, log) = run("create");
        {
            let mut config = backend.config.write().await;
            config.workspace_root = workspace.clone();
            config.host_workspace_root = workspace.clone();
        }
        let created = ensure_execution_environment(&backend, &profile, request())
            .await
            .expect("scoped create must succeed");
        assert_eq!(created.workspace_binding_id, Some(5));
        let logged = std::fs::read_to_string(&log).expect("create log");
        assert!(logged.contains("--mount type=bind"));
        assert!(logged.contains("--workdir /workspace"));
        assert!(logged.contains("-e DF_STATE_DIRECTORY=/state"));
        assert!(logged.contains(&image));
        assert!(logged.contains("--network df-private"));
        assert!(
            logged.contains("inspect scoped-helper-id")
                || logged.contains(&format!("inspect {container}")),
            "post-create identity must be inspected: {logged}"
        );
        let lines: Vec<&str> = logged.lines().collect();
        let prep_index = lines
            .iter()
            .position(|line| line.starts_with("run --rm "))
            .expect("scoped create must prepare the state volume ownership");
        let prep = lines[prep_index];
        assert!(
            prep.contains(&format!("--mount type=volume,src={volume},dst=/state")),
            "ownership prep must mount only the derived state volume: {prep}"
        );
        assert!(prep.contains("--cap-add CHOWN"), "got: {prep}");
        assert!(prep.contains("--cap-add FOWNER"), "got: {prep}");
        assert!(prep.contains("--user 0:0"), "got: {prep}");
        assert!(!prep.contains("type=bind"), "no workspace bind: {prep}");
        assert!(!prep.contains("--env-file"), "no credential file: {prep}");
        assert!(!prep.contains("-R"), "no recursive ownership: {prep}");
        let helper_index = lines
            .iter()
            .position(|line| line.starts_with("run ") && line.contains("--env-file"))
            .expect("scoped create must launch the helper");
        assert!(
            prep_index < helper_index,
            "state ownership must complete before the helper launch: {logged}"
        );

        let (backend, log) = run("reuse");
        {
            let mut config = backend.config.write().await;
            config.workspace_root = workspace.clone();
            config.host_workspace_root = workspace.clone();
        }
        let reused = ensure_execution_environment(&backend, &profile, request())
            .await
            .expect("same-generation scoped reuse must succeed");
        assert_eq!(reused.workspace_binding_id, Some(5));
        let logged = std::fs::read_to_string(&log).expect("reuse log");
        assert_eq!(
            logged
                .lines()
                .filter(|line| line.starts_with("run --rm "))
                .count(),
            1,
            "reuse must still prepare only its own derived state volume: {logged}"
        );
        assert!(
            !logged
                .lines()
                .any(|line| line.starts_with("run ") && line.contains("--env-file")),
            "reuse must not create a second helper: {logged}"
        );

        unsafe {
            std::env::remove_var(crate::runtime::workspace_service::WORKSPACE_SERVICE_IMAGE_ENV);
        }
    }

    #[tokio::test]
    async fn same_generation_is_idempotent_without_recreation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(3);
        let container = owner.container_name();
        let owner_key = owner.stable_key();
        let ps_file = temp.path().join("ps.txt");
        std::fs::write(&ps_file, format!("{container}\n")).expect("ps");
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("running", &owner_key, support::ENV_ID, "3"),
        )
        .expect("inspect");
        let log = temp.path().join("ensure-idem.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  name=\"$2\"\n  if [ \"$name\" = \"{}\" ]; then\n    cat \"{}\"\n    exit 0\n  fi\n  echo \"Error: No such container: $name\" >&2\n  exit 1\nfi\nif [ \"$1\" = \"volume\" ]; then\n  exit 0\nfi\nexit 0\n",
            log.display(),
            ps_file.display(),
            container,
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let ack = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            EnsureExecutionEnvironmentRequest {
                owner: owner.clone(),
                workspace_scope: None,
            },
        )
        .await
        .expect("same generation running must be idempotent");
        assert_eq!(ack.owner, owner);
        assert_eq!(ack.container_name, container);
        assert_eq!(ack.state, "running");
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(logged.contains("inspect"), "must inspect, got: {logged}");
        assert!(
            !logged.lines().any(|l| l.starts_with("run ")),
            "idempotent ensure must not docker run, got: {logged}"
        );
    }

    #[tokio::test]
    async fn old_running_generation_refuses_without_deletion() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let old_owner = support::env_owner(0);
        let old_container = old_owner.container_name();
        let old_key = old_owner.stable_key();
        let new_owner = support::env_owner(1);
        let ps_file = temp.path().join("ps.txt");
        std::fs::write(&ps_file, format!("{old_container}\n")).expect("ps");
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("running", &old_key, support::ENV_ID, "0"),
        )
        .expect("inspect");
        let log = temp.path().join("refuse.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"volume\" ]; then\n  exit 0\nfi\nexit 0\n",
            log.display(),
            ps_file.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let result = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            EnsureExecutionEnvironmentRequest {
                owner: new_owner,
                workspace_scope: None,
            },
        )
        .await;
        assert!(result.is_err(), "old running generation must refuse");
        assert!(
            result.unwrap_err().to_string().contains("refused"),
            "error must say refused"
        );
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged.lines().any(|l| l.starts_with("run ")),
            "refusal must not create, got: {logged}"
        );
        assert!(
            !logged.contains(" rm"),
            "must never delete to force ensure, got: {logged}"
        );
        // An old native writer still holds state: it blocks the new
        // generation until an explicit owner-verified stop, and its state
        // directory is never probed as the new owner's.
        assert!(
            !logged.lines().any(|l| l.starts_with("exec ")),
            "the blocking old writer must not be probed, got: {logged}"
        );
    }

    #[tokio::test]
    async fn unknown_other_generation_refuses_creation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let old_owner = support::env_owner(0);
        let old_container = old_owner.container_name();
        let old_key = old_owner.stable_key();
        let ps_file = temp.path().join("ps.txt");
        std::fs::write(&ps_file, format!("{old_container}\n")).expect("ps");
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("unknown", &old_key, support::ENV_ID, "0"),
        )
        .expect("inspect");
        let log = temp.path().join("unknown.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"volume\" ]; then\n  exit 0\nfi\nexit 0\n",
            log.display(),
            ps_file.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let result = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(2),
                workspace_scope: None,
            },
        )
        .await;
        assert!(result.is_err(), "unknown generation must refuse");
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(!logged.lines().any(|l| l.starts_with("run ")));
    }

    #[tokio::test]
    async fn malformed_owners_are_rejected_without_docker_calls() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("malformed.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let bad = vec![
            RunnerOwner::InternalUser { user_id: 1 },
            RunnerOwner::WorkspaceBinding {
                workspace_binding_id: 1,
            },
            RunnerOwner::ExecutionEnvironment {
                environment_id: "not-a-uuid".to_string(),
                generation: 0,
            },
            RunnerOwner::ExecutionEnvironment {
                environment_id: support::ENV_ID.to_string(),
                generation: -1,
            },
        ];
        for owner in bad {
            let result = ensure_execution_environment(
                &backend,
                &support::bare_enabled_profile(),
                EnsureExecutionEnvironmentRequest {
                    owner,
                    workspace_scope: None,
                },
            )
            .await;
            assert!(result.is_err(), "malformed owner must be rejected");
        }
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(logged.is_empty(), "rejected owners must not touch docker");
    }

    #[test]
    fn volume_name_stable_and_locks_are_environment_level() {
        let id = support::ENV_ID;
        let first =
            runner_protocol::execution_environment::derive_state_volume_name(id).expect("volume");
        let owner0 = support::env_owner(0);
        let owner1 = support::env_owner(7);
        let second =
            runner_protocol::execution_environment::derive_state_volume_name(id).expect("volume");
        assert_eq!(first, second);
        assert_eq!(owner0.lock_key(), owner1.lock_key());
        assert_ne!(owner0.stable_key(), owner1.stable_key());
        assert_ne!(owner0.container_name(), owner1.container_name());
        assert!(owner0.container_name().contains('0'));
    }

    #[test]
    fn legacy_owner_keys_are_unchanged() {
        let user = RunnerOwner::InternalUser { user_id: 1 };
        assert_eq!(user.stable_key(), "user:1");
        assert_eq!(user.container_name(), "desk-foreman-runner-user-1");
        let binding = RunnerOwner::WorkspaceBinding {
            workspace_binding_id: 42,
        };
        assert_eq!(binding.stable_key(), "workspace_binding:42");
        assert_eq!(
            binding.container_name(),
            "desk-foreman-runner-workspace_binding-42"
        );
    }

    #[tokio::test]
    async fn unusable_creation_profile_refuses_ensure_before_docker_and_keeps_stop_working() {
        // Requirement (749 P1): a profile that cannot launch anything must
        // refuse every ensure branch, including reuse, before any Docker
        // or disk work, while stop stays operable.
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("invalid-profile.log");
        std::fs::write(&log, "").expect("seed log");
        let owner = support::env_owner(0);
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("exited", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect json");
        let removed = temp.path().join("removed.marker");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  if [ -f \"{}\" ]; then\n    echo \"Error: No such container: $2\" >&2\n    exit 1\n  fi\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"rm\" ]; then\n  touch \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            removed.display(),
            inspect_file.display(),
            removed.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let enabled = support::enabled_profile(&state);

        let unusable = [
            ("default", ExecutionEnvironmentProfile::default()),
            (
                "unset command",
                ExecutionEnvironmentProfile {
                    command: TrustedCommand::Unset,
                    ..enabled.clone()
                },
            ),
            (
                "malformed command",
                ExecutionEnvironmentProfile {
                    command: parse_trusted_command(Some("not-json")),
                    ..enabled.clone()
                },
            ),
            (
                "unset workdir",
                ExecutionEnvironmentProfile {
                    state_workdir: TrustedStateWorkdir::Unset,
                    ..enabled.clone()
                },
            ),
            (
                "escaping workdir",
                ExecutionEnvironmentProfile {
                    state_workdir: parse_state_workdir(Some("/state/../etc"), "/state"),
                    ..enabled.clone()
                },
            ),
            (
                "root uid",
                ExecutionEnvironmentProfile {
                    volume_uid: Some(0),
                    ..enabled.clone()
                },
            ),
            (
                "absent ownership",
                ExecutionEnvironmentProfile {
                    volume_uid: None,
                    volume_gid: None,
                    ..enabled.clone()
                },
            ),
        ];

        for (label, profile) in unusable {
            // Give the default profile the same durable root so the refusal
            // can only come from the creation inputs.
            let profile = ExecutionEnvironmentProfile {
                state_dir: Some(state.clone()),
                enabled: true,
                ..profile
            };
            let ensure = ensure_execution_environment(
                &backend,
                &profile,
                EnsureExecutionEnvironmentRequest {
                    owner: owner.clone(),
                    workspace_scope: None,
                },
            )
            .await;
            assert!(ensure.is_err(), "{label} profile must refuse ensure");
            assert!(
                !ensure.unwrap_err().to_string().contains("not-json"),
                "{label} refusal must stay redacted"
            );
            assert!(
                std::fs::read_to_string(&log).expect("read log").is_empty(),
                "{label} profile must not touch docker"
            );
        }

        // Stop keeps working on the very same unlaunchable profile.
        let unlaunchable = ExecutionEnvironmentProfile {
            command: TrustedCommand::Unset,
            state_workdir: TrustedStateWorkdir::Unset,
            volume_uid: None,
            volume_gid: None,
            ..enabled
        };
        let ack = stop_execution_environment(
            &backend,
            &unlaunchable,
            StopExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("stop stays operable without creation inputs");
        assert!(ack.confirmed);
    }

    #[tokio::test]
    async fn direct_backend_reports_unsupported_clearly() {
        let backend = crate::runtime::DirectRunnerBackend::new();
        let owner = support::env_owner(0);
        let result = backend
            .ensure_execution_environment(EnsureExecutionEnvironmentRequest {
                owner,
                workspace_scope: None,
            })
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not supported"),
            "unsupported backend must be explicit"
        );
    }

    #[tokio::test]
    async fn environment_locks_serialize_across_generations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("locks.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let a = support::env_owner(0);
        let b = support::env_owner(1);
        let la = backend.runner_lock(&a);
        let lb = backend.runner_lock(&b);
        assert!(Arc::ptr_eq(&la, &lb), "same env must share one lock");
    }
}
