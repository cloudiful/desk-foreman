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
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use runner_protocol::{
        EnsureExecutionEnvironmentRequest, RunnerOwner, StopExecutionEnvironmentRequest,
    };

    use super::super::{
        ExecutionEnvironmentProfile,
        ensure::ensure_execution_environment,
        service::{
            ServiceProfile, credential::ServiceCredentialStore, profile::ServiceProfileConfig,
        },
        stop::stop_execution_environment,
        test_support as support,
    };
    use crate::runtime::RunnerBackend;

    const ENV_NAME: &str = "DF_AGENT_TOKEN";

    fn enabled(state_root: &Path) -> ExecutionEnvironmentProfile {
        support::enabled_profile(state_root)
    }

    fn disabled(state_root: &Path) -> ExecutionEnvironmentProfile {
        support::disabled_profile(state_root)
    }

    fn configured(state_root: &Path) -> ExecutionEnvironmentProfile {
        let mut profile = enabled(state_root);
        profile.service = ServiceProfile::Configured(ServiceProfileConfig {
            network: "df-private".to_string(),
            port: 8443,
            auth_username: "agent".to_string(),
            auth_env_name: ENV_NAME.to_string(),
        });
        profile
    }

    fn seed_credential(state_root: &Path, generation: i64) -> std::path::PathBuf {
        ServiceCredentialStore::new(state_root)
            .obtain(support::ENV_ID, generation, ENV_NAME)
            .expect("seed service credential")
            .env_file()
            .to_path_buf()
    }

    fn absent_backend(
        temp: &Path,
        name: &str,
    ) -> std::sync::Arc<crate::runtime::DockerRunnerBackend> {
        let log = temp.join("stop-absent.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  echo \"Error: No such container: $2\" >&2\n  exit 1\nfi\nexit 0\n",
            log.display()
        );
        let fake = support::write_executable(temp, name, &script);
        support::backend_with_cli(&fake.to_string_lossy())
    }

    /// Fake with a real container id that reports `exited` until `rm` marks it
    /// removed, after which both id and name inspections report absence.
    fn exited_until_removed_backend(
        temp: &Path,
        name: &str,
        owner: &RunnerOwner,
        generation: i64,
    ) -> (
        std::sync::Arc<crate::runtime::DockerRunnerBackend>,
        std::path::PathBuf,
        String,
    ) {
        let container_id = format!("{name}-container-id");
        let inspect = temp.join(format!("{name}.exited.json"));
        std::fs::write(
            &inspect,
            support::inspect_json_with_id(
                "exited",
                &container_id,
                &owner.stable_key(),
                support::ENV_ID,
                &generation.to_string(),
            ),
        )
        .expect("exited json");
        let removed = temp.join(format!("{name}.removed.marker"));
        let log = temp.join(format!("{name}.log"));
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  if [ -f \"{}\" ]; then\n    echo \"Error: No such container: $2\" >&2\n    exit 1\n  fi\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"rm\" ]; then\n  touch \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            removed.display(),
            inspect.display(),
            removed.display()
        );
        let fake = support::write_executable(temp, name, &script);
        (
            support::backend_with_cli(&fake.to_string_lossy()),
            log,
            container_id,
        )
    }

    #[tokio::test]
    async fn disabled_profile_permits_owner_validated_stop_and_rejects_ensure() {
        // Requirement change (740 P1): stop is cleanup, not a creation
        // authorization shortcut, so a disabled profile must still stop an
        // existing owner-validated environment while ensure stays refused.
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let (backend, _log, _) =
            exited_until_removed_backend(temp.path(), "docker-disabled-stop", &owner, 0);

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
        assert_eq!(ack.observed_state, "absent");

        let ensure = ensure_execution_environment(
            &backend,
            &disabled(&state),
            EnsureExecutionEnvironmentRequest {
                owner,
                workspace_scope: None,
            },
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
        let credential = seed_credential(&state, 0);
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
            &configured(&state),
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
        assert!(
            !credential.exists(),
            "a confirmed absent stop retires its exact credential"
        );
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
    async fn stop_exited_container_removes_by_id_and_confirms_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(1);
        let credential = seed_credential(&state, 1);
        let container = owner.container_name();
        let (backend, log, container_id) =
            exited_until_removed_backend(temp.path(), "docker-exited", &owner, 1);
        let ack = stop_execution_environment(
            &backend,
            &configured(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("an exited container must be removed and confirm absence");
        assert!(ack.confirmed);
        assert_eq!(ack.observed_state, "absent");
        assert_eq!(ack.owner, owner);
        assert!(
            !credential.exists(),
            "a confirmed absent stop retires its exact credential"
        );
        let logged = std::fs::read_to_string(&log).expect("read log");
        for expected in [format!("stop {container_id}"), format!("rm {container_id}")] {
            assert!(
                logged.lines().any(|line| line == expected),
                "expected `{expected}` in: {logged}"
            );
        }
        for expected in [
            format!("inspect {container_id} "),
            format!("inspect {container} "),
        ] {
            assert!(
                logged
                    .lines()
                    .any(|line| line.starts_with(expected.as_str())),
                "expected `{expected}` in: {logged}"
            );
        }
        assert!(
            !logged.lines().any(
                |line| line == format!("stop {container}") || line == format!("rm {container}")
            ),
            "the canonical name must never be the destructive target: {logged}"
        );
        assert!(
            !logged
                .lines()
                .any(|line| line == format!("rm -f {container_id}")),
            "a successful rm needs no force escalation: {logged}"
        );
    }

    #[tokio::test]
    async fn stop_remaining_exited_after_unknown_rm_is_unconfirmed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(3);
        let credential = seed_credential(&state, 3);
        let container_id = "stop-leftover-id";
        let inspect_file = temp.path().join("leftover.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json_with_id(
                "exited",
                container_id,
                &owner.stable_key(),
                support::ENV_ID,
                "3",
            ),
        )
        .expect("inspect");
        let log = temp.path().join("stop-leftover.log");
        std::fs::write(&log, "").expect("seed log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"stop\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"rm\" ] && [ \"$2\" = \"-f\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"rm\" ]; then\n  echo \"rm transport unknown\" >&2\n  exit 1\nfi\nexit 0\n",
            log.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-leftover", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let ack = stop_execution_environment(
            &backend,
            &configured(&state),
            StopExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("a remaining exited object is an observed ack, not an error");
        assert!(
            !ack.confirmed,
            "a matching object that still exists is not absence"
        );
        assert_eq!(ack.observed_state, "exited");
        assert!(
            credential.exists(),
            "an unconfirmed stop never retires the credential"
        );
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            logged
                .lines()
                .any(|line| line == format!("rm -f {container_id}")),
            "an unknown rm escalates to force removal by id: {logged}"
        );
    }

    #[tokio::test]
    async fn stop_daemon_failure_never_confirms() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let credential = seed_credential(&state, 0);
        let log = temp.path().join("stop-fail.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-fail", &log, "fail");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let result = stop_execution_environment(
            &backend,
            &configured(&state),
            StopExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await;
        assert!(result.is_err(), "daemon failure must fail closed");
        assert!(
            credential.exists(),
            "a daemon failure never retires the credential"
        );
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(!logged.contains("volume rm"));
    }

    #[tokio::test]
    async fn stop_hanging_daemon_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let credential = seed_credential(&state, 0);
        let log = temp.path().join("stop-hang.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-hang", &log, "hang");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let mut profile = configured(&state);
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
        assert!(
            credential.exists(),
            "a bounded timeout never retires the credential"
        );
    }

    #[tokio::test]
    async fn stop_invalid_inspect_json_cannot_confirm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let credential = seed_credential(&state, 0);
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
            &configured(&state),
            StopExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await;
        assert!(result.is_err(), "invalid JSON must not confirm");
        assert!(
            credential.exists(),
            "an unconfirmed inspection never retires the credential"
        );
    }

    #[tokio::test]
    async fn stop_running_after_stop_is_unconfirmed_not_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let credential = seed_credential(&state, 0);
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
            &configured(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("running inspect must return ack");
        assert!(!ack.confirmed, "running must not confirm stop");
        assert_eq!(ack.observed_state, "running");
        assert_eq!(ack.owner, owner);
        assert!(
            credential.exists(),
            "an unconfirmed stop never retires the credential"
        );
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

    #[tokio::test]
    async fn unset_retains_sibling_credentials_and_a_configured_retry_removes_only_its_generation()
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let retired = seed_credential(&state, 0);
        let sibling = seed_credential(&state, 1);
        let backend = absent_backend(temp.path(), "docker-unset");

        let unset = stop_execution_environment(
            &backend,
            &enabled(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect("an unset profile still confirms the stop");
        assert!(unset.confirmed);
        assert!(
            retired.exists() && sibling.exists(),
            "an unset service profile deletes no credential and makes no cleanup claim"
        );

        let configured_ack = stop_execution_environment(
            &backend,
            &configured(&state),
            StopExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("a configured profile retires the confirmed generation");
        assert!(configured_ack.confirmed);
        assert!(!retired.exists(), "the exact generation is unlinked");
        assert!(sibling.exists(), "a sibling generation stays untouched");
        let barrier = support::read_barrier(&state).expect("barrier");
        assert_eq!(barrier["stopped_generation"], serde_json::json!(0));
        assert!(
            !support::lock_path(&state).exists(),
            "the file lock is released after retirement"
        );
    }

    #[tokio::test]
    async fn a_failed_retirement_keeps_the_file_and_a_corrected_retry_removes_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let credential = seed_credential(&state, 0);
        let service_dir = credential
            .parent()
            .expect("service directory")
            .to_path_buf();
        std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o644))
            .expect("widen credential mode");
        let backend = absent_backend(temp.path(), "docker-invalid-credential");

        let error = stop_execution_environment(
            &backend,
            &configured(&state),
            StopExecutionEnvironmentRequest {
                owner: owner.clone(),
            },
        )
        .await
        .expect_err("a failed retirement surfaces after the container stop");
        assert!(error.to_string().contains("0600"), "got: {error}");
        assert!(credential.exists(), "a refused credential is not removed");
        let barrier = support::read_barrier(&state).expect("the stop persisted its barrier");
        assert_eq!(barrier["stopped_generation"], serde_json::json!(0));
        assert!(
            !support::lock_path(&state).exists(),
            "a failed retirement still releases the file lock"
        );

        std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600))
            .expect("correct the fixture mode");
        let retry = stop_execution_environment(
            &backend,
            &configured(&state),
            StopExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("an absent-confirm retry removes the corrected credential");
        assert!(retry.confirmed);
        assert!(
            !credential.exists(),
            "the corrected credential is retried and removed"
        );
        assert_eq!(
            std::fs::read_dir(&service_dir)
                .expect("service directory")
                .count(),
            0,
            "no temporary or credential file is left behind"
        );
    }
}
