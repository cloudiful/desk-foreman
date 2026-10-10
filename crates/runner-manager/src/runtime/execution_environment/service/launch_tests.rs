//! Service-enabled ensure integration tests (issue 749 P2a,
//! `#[cfg(test)]`).
//!
//! Drives the real ensure path through the fake Docker CLI to prove the
//! launch contract: a configured service injects the network and the
//! credential env-file and nothing else, reuse refuses a container whose
//! credential has gone missing, a degraded profile refuses before any
//! Docker call, an absent service leaves the previous behaviour untouched,
//! and stop stays operable either way. No live daemon.

#[cfg(test)]
mod launch_tests {
    use runner_protocol::{EnsureExecutionEnvironmentRequest, StopExecutionEnvironmentRequest};

    use std::path::Path;
    use std::sync::Arc;

    use super::super::super::test_support as support;
    use super::super::super::{
        ExecutionEnvironmentProfile, ensure::ensure_execution_environment,
        stop::stop_execution_environment,
    };
    use super::super::credential::ServiceCredentialStore;
    use super::super::profile::{ServiceProfile, ServiceProfileEnv, parse_service_profile};
    use crate::runtime::docker::DockerRunnerBackend;
    use std::os::unix::fs::PermissionsExt;

    const ENV_NAME: &str = "DF_AGENT_TOKEN";

    fn serving() -> ServiceProfile {
        parse_service_profile(ServiceProfileEnv {
            network: Some("df-private"),
            port: Some("8443"),
            auth_username: Some("agent"),
            auth_env_name: Some(ENV_NAME),
        })
    }

    fn profile_with(state_root: &Path, service: ServiceProfile) -> ExecutionEnvironmentProfile {
        ExecutionEnvironmentProfile {
            service,
            ..support::enabled_profile(state_root)
        }
    }

    /// Fake that reports no pre-existing container, accepts `run`, and
    /// reports the requested owner `running` on inspect.
    fn creation_backend(
        dir: &Path,
        log: &Path,
        generation: i64,
    ) -> (Arc<DockerRunnerBackend>, std::path::PathBuf) {
        let owner = support::env_owner(generation);
        let running_file = dir.join("serve.running.json");
        std::fs::write(
            &running_file,
            support::inspect_json(
                "running",
                &owner.stable_key(),
                support::ENV_ID,
                &generation.to_string(),
            ),
        )
        .expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            running_file.display()
        );
        let fake = support::write_executable(dir, "docker-fake", &script);
        (
            support::backend_with_cli(&fake.to_string_lossy()),
            support::state_dir(dir),
        )
    }

    #[tokio::test]
    async fn a_served_environment_receives_the_network_and_credential_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("serve.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, state) = creation_backend(temp.path(), &log, 0);

        ensure_execution_environment(
            &backend,
            &profile_with(&state, serving()),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await
        .expect("a served environment must launch");

        let logged = std::fs::read_to_string(&log).expect("read log");
        let run_line = logged
            .lines()
            .find(|line| line.starts_with("run "))
            .expect("run issued");
        assert!(run_line.contains("--network df-private"), "got: {run_line}");
        assert!(run_line.contains("--env-file "), "got: {run_line}");
        assert!(!run_line.contains("--network none"), "one network wins");
        assert!(
            !run_line.contains(" -p ") && !run_line.contains(" --publish"),
            "a private service is never published"
        );
        // The argv carries the credential's host path, never its value.
        let env_file = run_line
            .split("--env-file ")
            .nth(1)
            .expect("env-file present")
            .split_whitespace()
            .next()
            .expect("env-file path");
        let contents = std::fs::read_to_string(env_file).expect("credential file");
        let token = contents.trim_end().split_once('=').expect("NAME=token").1;
        assert!(
            !run_line.contains(token),
            "the credential value must never reach the argv"
        );
        assert!(run_line.contains(env_file), "only the path is passed");
    }

    #[tokio::test]
    async fn the_credential_lands_under_the_manager_state_root_as_a_private_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("stable.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, state) = creation_backend(temp.path(), &log, 0);

        ensure_execution_environment(
            &backend,
            &profile_with(&state, serving()),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await
        .expect("first launch");

        let path = ServiceCredentialStore::new(&state)
            .credential_path(support::ENV_ID, 0)
            .expect("credential path");
        assert!(
            path.starts_with(&state),
            "it lives in the manager state root"
        );
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the credential must be owner-only");
        assert!(
            !path.starts_with(temp.path().join("agent")),
            "the credential is never placed on a container path"
        );
    }

    #[tokio::test]
    async fn reuse_fails_closed_when_the_credential_is_gone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("reuse.log");
        std::fs::write(&log, "").expect("seed log");
        let owner = support::env_owner(0);
        let ps_file = temp.path().join("ps.txt");
        std::fs::write(&ps_file, format!("{}\n", owner.container_name())).expect("ps");
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("running", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            ps_file.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let state = support::state_dir(temp.path());

        let result = ensure_execution_environment(
            &backend,
            &profile_with(&state, serving()),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await;
        let error = result
            .expect_err("a running container without a credential must not acknowledge")
            .to_string();
        assert!(error.contains("not available"), "got: {error}");
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged.lines().any(|line| line.starts_with("run ")),
            "reuse must never fall through to creation, got: {logged}"
        );
    }

    #[tokio::test]
    async fn a_degraded_service_profile_refuses_before_docker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("degraded.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let state = support::state_dir(temp.path());

        let degraded = parse_service_profile(ServiceProfileEnv {
            network: Some("df-private"),
            ..ServiceProfileEnv::default()
        });
        let result = ensure_execution_environment(
            &backend,
            &profile_with(&state, degraded),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await;
        assert!(
            result.is_err(),
            "a half-configured service must refuse ensure"
        );
        assert!(
            std::fs::read_to_string(&log).expect("read log").is_empty(),
            "the refusal must happen before any Docker work"
        );
    }

    #[tokio::test]
    async fn an_absent_service_profile_leaves_the_previous_behaviour_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("absent.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, state) = creation_backend(temp.path(), &log, 0);

        ensure_execution_environment(
            &backend,
            &profile_with(&state, ServiceProfile::Unset),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await
        .expect("an unserved environment must still launch");

        let logged = std::fs::read_to_string(&log).expect("read log");
        let run_line = logged
            .lines()
            .find(|line| line.starts_with("run "))
            .expect("run issued");
        assert!(run_line.contains("--network none"), "got: {run_line}");
        assert!(!run_line.contains("--env-file"));
        assert!(
            !state.join("service").exists(),
            "no credential directory is created when service support is off"
        );
    }

    #[tokio::test]
    async fn stop_stays_operable_for_a_served_environment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("stop.log");
        std::fs::write(&log, "").expect("seed log");
        let owner = support::env_owner(0);
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json("exited", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let state = support::state_dir(temp.path());

        let ack = stop_execution_environment(
            &backend,
            &profile_with(&state, serving()),
            StopExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("stop must not depend on the service profile");
        assert!(ack.confirmed);
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged.lines().any(|line| line.starts_with("exec ")),
            "stop never probes the agent service, got: {logged}"
        );
    }
}
