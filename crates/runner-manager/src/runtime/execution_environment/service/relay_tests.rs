//! Manager-side admission tests (issue 749 P2c, `#[cfg(test)]`).
//!
//! Fake Docker CLI plus a real state root: what these prove is that only an
//! owner-matching running container on the trusted network with a valid
//! credential is admitted, and that a foreign container, a closed
//! generation, or an absent container is refused before anything dials out.

#[cfg(test)]
mod relay_tests {
    use super::super::super::test_support;
    use super::super::credential::ServiceCredentialStore;
    use super::super::profile::ServiceProfileConfig;
    use super::super::relay::admit_service_stream;
    use crate::runtime::docker::DockerRunnerBackend;
    use std::sync::Arc;

    const ENV_NAME: &str = "DF_AGENT_TOKEN";

    fn service_profile(
        state_root: &std::path::Path,
    ) -> super::super::super::ExecutionEnvironmentProfile {
        let mut profile = test_support::enabled_profile(state_root);
        profile.service = super::super::profile::ServiceProfile::Configured(ServiceProfileConfig {
            network: "df-private".to_string(),
            port: 8443,
            auth_username: "agent".to_string(),
            auth_env_name: ENV_NAME.to_string(),
        });
        profile
    }

    fn networks() -> serde_json::Value {
        serde_json::json!({ "df-private": { "IPAddress": "10.0.0.5", "GlobalIPv6Address": "" } })
    }

    /// Fake whose every `inspect` reports the requested owner running on
    /// the trusted network; all other subcommands succeed silently.
    fn write_service_fake(
        dir: &std::path::Path,
        name: &str,
        log: &std::path::Path,
        owner_key: &str,
        generation: i64,
    ) -> std::path::PathBuf {
        let json_file = dir.join(format!("{name}.running.json"));
        std::fs::write(
            &json_file,
            test_support::inspect_json_with_networks(
                "running",
                owner_key,
                test_support::ENV_ID,
                &generation.to_string(),
                &networks(),
            ),
        )
        .expect("running json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            json_file.display()
        );
        test_support::write_executable(dir, name, &script)
    }

    fn backend(cli: &std::path::Path) -> Arc<DockerRunnerBackend> {
        test_support::backend_with_cli(cli.to_str().expect("cli path"))
    }

    #[tokio::test]
    async fn a_matching_running_container_with_a_credential_is_admitted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bin");
        std::fs::create_dir_all(&dir).expect("bin");
        let log = temp.path().join("docker.log");
        let owner = test_support::env_owner(4);
        let cli = write_service_fake(&dir, "docker", &log, &owner.stable_key(), 4);
        let backend = backend(&cli);
        let state_root = test_support::state_dir(temp.path());
        std::fs::create_dir_all(&state_root).expect("state");
        ServiceCredentialStore::new(&state_root)
            .obtain(test_support::ENV_ID, 4, ENV_NAME)
            .expect("credential");
        let profile = service_profile(&state_root);
        let admitted = admit_service_stream(&backend, &profile, test_support::ENV_ID, 4)
            .await
            .expect("matching running container serves");
        assert_eq!(admitted.target.to_string(), "10.0.0.5");
        assert_eq!(admitted.port, 8443);
        assert_eq!(admitted.username, "agent");
    }

    #[tokio::test]
    async fn a_foreign_container_is_never_admitted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bin");
        std::fs::create_dir_all(&dir).expect("bin");
        let log = temp.path().join("docker.log");
        let cli = write_service_fake(&dir, "docker", &log, "someone-else", 4);
        let backend = backend(&cli);
        let state_root = test_support::state_dir(temp.path());
        std::fs::create_dir_all(&state_root).expect("state");
        let profile = service_profile(&state_root);
        assert!(
            admit_service_stream(&backend, &profile, test_support::ENV_ID, 4)
                .await
                .is_err(),
            "foreign labels must refuse before any dial"
        );
    }

    #[tokio::test]
    async fn a_closed_generation_never_dials_out() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bin");
        std::fs::create_dir_all(&dir).expect("bin");
        let log = temp.path().join("docker.log");
        let owner = test_support::env_owner(4);
        let cli = write_service_fake(&dir, "docker", &log, &owner.stable_key(), 4);
        let backend = backend(&cli);
        let state_root = test_support::state_dir(temp.path());
        std::fs::create_dir_all(&state_root).expect("state");
        // Persist a stop for generation 4 first: the barrier is now closed.
        let profile = service_profile(&state_root);
        let stop_request = runner_protocol::StopExecutionEnvironmentRequest { owner };
        super::super::super::stop::stop_execution_environment(&backend, &profile, stop_request)
            .await
            .expect("stop persists");
        assert!(
            admit_service_stream(&backend, &profile, test_support::ENV_ID, 4)
                .await
                .is_err(),
            "a stopped generation must refuse without dialing"
        );
    }

    #[tokio::test]
    async fn a_committed_stop_wakes_a_live_stream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bin");
        std::fs::create_dir_all(&dir).expect("bin");
        let log = temp.path().join("docker.log");
        let owner = test_support::env_owner(4);
        let cli = write_service_fake(&dir, "docker", &log, &owner.stable_key(), 4);
        let backend = backend(&cli);
        let state_root = test_support::state_dir(temp.path());
        std::fs::create_dir_all(&state_root).expect("state");
        let mut receiver = backend.service_cancel().register(test_support::ENV_ID);
        let profile = service_profile(&state_root);
        let stop_request = runner_protocol::StopExecutionEnvironmentRequest { owner };
        super::super::super::stop::stop_execution_environment(&backend, &profile, stop_request)
            .await
            .expect("committed stop");
        receiver
            .wait_for(|cancelled| *cancelled)
            .await
            .expect("a committed stop wakes its environment's streams");
    }

    #[tokio::test]
    async fn an_absent_container_is_refused_never_created() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bin");
        std::fs::create_dir_all(&dir).expect("bin");
        let log = temp.path().join("docker.log");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  echo \"Error: No such container\" >&2\n  exit 1\nfi\nexit 0\n",
            log.display()
        );
        let cli = test_support::write_executable(&dir, "docker", &script);
        let backend = backend(&cli);
        let state_root = test_support::state_dir(temp.path());
        std::fs::create_dir_all(&state_root).expect("state");
        let profile = service_profile(&state_root);
        assert!(
            admit_service_stream(&backend, &profile, test_support::ENV_ID, 4)
                .await
                .is_err(),
            "an absent container must be refused, never started for a relay"
        );
        let logged = std::fs::read_to_string(&log).expect("log");
        assert!(
            !logged.lines().any(|line| line.starts_with("run ")),
            "no container may be created for a relayed request, got: {logged}"
        );
    }
}
