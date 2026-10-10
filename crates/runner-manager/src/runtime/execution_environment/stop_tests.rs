//! Stop-path fake-Docker tests (issue 739 P1, durable barrier in 742 P1,
//! `#[cfg(test)]`).
//!
//! Proves confirmed stop only after inspect proves `exited`/`dead`/`removed`
//! or positively identified absence, with transport/parse/timeout/permission
//! failures never confirming. The durable stop barrier is written before the
//! destructive step, so a failed stop still closes the generation while
//! never confirming. Volume is never deleted. No live daemon.

#[cfg(test)]
mod stop_tests {
    use std::path::Path;

    use runner_protocol::{
        EnsureExecutionEnvironmentRequest, RunnerOwner, StopExecutionEnvironmentRequest,
    };

    use super::super::{
        ExecutionEnvironmentProfile, ensure::ensure_execution_environment,
        stop::stop_execution_environment, test_support as support,
    };
    use crate::runtime::RunnerBackend;

    fn enabled(state_root: &Path) -> ExecutionEnvironmentProfile {
        support::enabled_profile(state_root)
    }

    fn disabled(state_root: &Path) -> ExecutionEnvironmentProfile {
        support::disabled_profile(state_root)
    }

    #[tokio::test]
    async fn disabled_profile_permits_owner_validated_stop_and_rejects_ensure() {
        // Requirement change (740 P1): stop is cleanup, not a creation
        // authorization shortcut, so a disabled profile must still stop an
        // existing owner-validated environment while ensure stays refused.
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("exited", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect");
        let log = temp.path().join("stop-disabled.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let ack = stop_execution_environment(
            &backend,
            &disabled(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("disabled profile must still stop an existing environment");
        assert!(ack.confirmed);
        assert_eq!(ack.observed_state, "exited");

        let ensure = ensure_execution_environment(
            &backend,
            &disabled(&state),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await;
        assert!(ensure.is_err(), "disabled profile must still reject ensure");
        assert!(ensure.unwrap_err().to_string().contains("disabled"));
    }

    #[tokio::test]
    async fn stop_missing_container_confirms_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let container = owner.container_name();
        let log = temp.path().join("stop-absent.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  echo \"Error: No such container: $2\" >&2\n  exit 1\nfi\nexit 0\n",
            log.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let ack = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("missing must confirm absent");
        assert!(ack.confirmed);
        assert_eq!(ack.observed_state, "absent");
        assert_eq!(ack.owner, owner);
        assert_eq!(ack.container_name, container);
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged
                .lines()
                .any(|l| l.starts_with("stop ") || l.starts_with("rm ")),
            "positively absent must confirm with no destructive command, got: {logged}"
        );
        assert!(!logged.contains("volume rm"), "volume must survive stop");
        // Absent observation still durably records the stop barrier before
        // confirming.
        let barrier = support::read_barrier(&state).expect("absent stop must persist barrier");
        assert_eq!(barrier["stopped_generation"], serde_json::json!(0));
    }

    #[tokio::test]
    async fn stop_exited_container_confirms_stopped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(1);
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("exited", &owner.stable_key(), support::ENV_ID, "1"),
        )
        .expect("inspect");
        let log = temp.path().join("stop-exited.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let ack = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("exited must confirm");
        assert!(ack.confirmed);
        assert_eq!(ack.observed_state, "exited");
        assert_eq!(ack.owner, owner);
    }

    #[tokio::test]
    async fn stop_daemon_failure_never_confirms() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("stop-fail.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-fail", &log, "fail");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let result = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await;
        assert!(result.is_err(), "daemon failure must fail closed");
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(!logged.contains("volume rm"));
    }

    #[tokio::test]
    async fn stop_hanging_daemon_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("stop-hang.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-hang", &log, "hang");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let mut profile = enabled(&state);
        profile.stop_timeout = std::time::Duration::from_millis(80);
        let result = stop_execution_environment(
            &backend,
            &profile,
            StopExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn stop_invalid_inspect_json_cannot_confirm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("stop-badjson.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  echo \"not-json\"\n  exit 0\nfi\nexit 0\n",
            log.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let result = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await;
        assert!(result.is_err(), "invalid JSON must not confirm");
    }

    #[tokio::test]
    async fn stop_running_after_stop_is_unconfirmed_not_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("running", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect");
        let log = temp.path().join("stop-running.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let ack = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("running inspect must return ack");
        assert!(!ack.confirmed, "running must not confirm stop");
        assert_eq!(ack.observed_state, "running");
        assert_eq!(ack.owner, owner);
    }

    #[tokio::test]
    async fn failed_stop_leaves_barrier_without_confirmation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(2);
        let log = temp.path().join("stop-destroy-fail.log");
        std::fs::write(&log, "").expect("seed log");
        let running_file = temp.path().join("running.json");
        std::fs::write(
            &running_file,
            support::inspect_json("running", &owner.stable_key(), support::ENV_ID, "2"),
        )
        .expect("inspect");
        // Inspect always reports a matching running owner; every destructive
        // command fails, so stop must error without any confirmation.
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"stop\" ]; then\n  echo \"stop failed\" >&2\n  exit 1\nfi\nif [ \"$1\" = \"rm\" ]; then\n  echo \"rm failed\" >&2\n  exit 1\nfi\nexit 0\n",
            log.display(),
            running_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fail-destroy", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let result = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest { owner },
        )
        .await;
        assert!(result.is_err(), "failed destruction must not confirm");
        assert!(result.unwrap_err().to_string().contains("failed to remove"));
        // Barrier persisted before the destructive attempt, so the generation
        // is already closed even though stop never confirmed.
        let barrier = support::read_barrier(&state).expect("failed stop must persist barrier");
        assert_eq!(barrier["stopped_generation"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn stop_rejects_malformed_owners_and_reports_unsupported_backend() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("stop-malformed.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        for owner in [
            RunnerOwner::InternalUser { user_id: 1 },
            RunnerOwner::ExecutionEnvironment {
                environment_id: "bad".to_string(),
                generation: 0,
            },
        ] {
            let result = stop_execution_environment(
                &backend,
                &enabled(&state),
                StopExecutionEnvironmentRequest { owner },
            )
            .await;
            assert!(result.is_err());
        }
        let direct = crate::runtime::DirectRunnerBackend::new();
        let result = direct
            .stop_execution_environment(StopExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not supported"));
    }
}
