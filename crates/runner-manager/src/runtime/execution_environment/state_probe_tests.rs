//! State probe argv and behaviour tests (issue 749 P1, `#[cfg(test)]`).
//!
//! Proves the probe runs as the exact non-root owner inside the exact
//! owner container and working directory, that a failing or hanging probe
//! fails closed without acknowledging `running`, and that a failure never
//! cleans anything up. Fake CLI only; no live daemon.

#[cfg(test)]
mod state_probe_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use runner_protocol::{EnsureExecutionEnvironmentRequest, StopExecutionEnvironmentRequest};

    use super::super::{
        deadline::OperationDeadline,
        ensure::ensure_execution_environment,
        profile::{command::TrustedCommand, workdir::TrustedStateWorkdir},
        state_probe::{StateProbeSpec, build_state_probe_argv, confirm_state_writable},
        stop::stop_execution_environment,
        test_support as support,
    };
    use crate::runtime::docker::DockerRunnerBackend;

    fn spec<'a>(container_name: &'a str) -> StateProbeSpec<'a> {
        StateProbeSpec {
            container_name,
            state_workdir: "/state/work",
            uid: 1000,
            gid: 1000,
        }
    }

    #[test]
    fn probe_argv_targets_the_exact_container_user_and_workdir() {
        let argv = build_state_probe_argv(&spec("df-env-abc-0"));
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--user");
        assert_eq!(argv[2], "1000:1000");
        assert_eq!(argv[3], "--workdir");
        assert_eq!(argv[4], "/state/work");
        assert_eq!(argv[5], "df-env-abc-0", "exact owner container name");
        assert_eq!(argv[6], "sh");
        assert_eq!(argv[7], "-c");
        let script = argv[8].as_str();
        assert!(script.contains("mktemp"), "must create a real file");
        assert!(script.contains("trap"), "must clean up on any exit");
        assert!(
            !script.contains('\n'),
            "a single-line script keeps one invocation per logged command"
        );
        for forbidden in [
            "run ",
            "create ",
            "--privileged",
            "chown",
            "rm -rf",
            "/state/work",
            "1000:1000",
        ] {
            assert!(
                !script.contains(forbidden),
                "no configured value or extra operation may reach the fixed script: {forbidden:?}"
            );
        }
        assert_eq!(argv.len(), 9, "no extra flags leak into the exec argv");
    }

    /// Fake reporting no existing container: `run` succeeds, `inspect`
    /// reports the requested owner `running`, and the `exec` probe outcome
    /// is baked in per test so parallel tests never share process env.
    fn creation_backend(
        dir: &Path,
        log: &Path,
        generation: i64,
        probe: &str,
    ) -> (Arc<DockerRunnerBackend>, PathBuf) {
        let owner = support::env_owner(generation);
        let running_file = dir.join("probe.running.json");
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
        let probe_branch = match probe {
            "fail" => "echo \"state dir is read-only\" >&2\nexit 1",
            "hang" => "sleep 30\nexit 0",
            _ => "exit 0",
        };
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"exec\" ]; then\n  {}\nfi\nexit 0\n",
            log.display(),
            running_file.display(),
            probe_branch
        );
        let fake = support::write_executable(dir, "docker-fake", &script);
        (
            support::backend_with_cli(&fake.to_string_lossy()),
            support::state_dir(dir),
        )
    }

    #[tokio::test]
    async fn creation_probes_after_owner_validation_and_before_acknowledging() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-create.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, state) = creation_backend(temp.path(), &log, 0, "ok");

        let ack = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await
        .expect("creation with a writable state dir must succeed");
        assert_eq!(ack.state, "running");

        let logged = std::fs::read_to_string(&log).expect("read log");
        let lines: Vec<&str> = logged.lines().collect();
        let run_index = lines
            .iter()
            .position(|line| line.starts_with("run "))
            .expect("run issued");
        let exec_index = lines
            .iter()
            .position(|line| line.starts_with("exec "))
            .expect("probe issued");
        assert!(exec_index > run_index, "probe must follow creation");
        assert!(
            logged.contains("--workdir /state/work"),
            "probe must use the configured workdir, got: {logged}"
        );
        assert!(
            logged.contains("--user 1000:1000"),
            "probe must use the configured non-root user, got: {logged}"
        );
        assert!(
            logged.contains("--workdir /state/work sleep infinity")
                || logged.contains("sleep infinity"),
            "the trusted command runs as the container command, got: {logged}"
        );
    }

    #[tokio::test]
    async fn reuse_probes_an_already_running_container() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-reuse.log");
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

        ensure_execution_environment(
            &backend,
            &support::enabled_profile(&support::state_dir(temp.path())),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("reuse with a writable state dir must succeed");

        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            logged.contains("exec --user 1000:1000 --workdir /state/work"),
            "a reused running container must still be probed, got: {logged}"
        );
        assert!(!logged.lines().any(|line| line.starts_with("run ")));
    }

    #[tokio::test]
    async fn restart_probes_after_the_owner_recheck() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-restart.log");
        std::fs::write(&log, "").expect("seed log");
        let owner = support::env_owner(0);
        let ps_file = temp.path().join("ps.txt");
        std::fs::write(&ps_file, format!("{}\n", owner.container_name())).expect("ps");
        let stopped_file = temp.path().join("stopped.json");
        std::fs::write(
            &stopped_file,
            support::inspect_json("exited", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect json");
        let running_file = temp.path().join("running.json");
        std::fs::write(
            &running_file,
            support::inspect_json("running", &owner.stable_key(), support::ENV_ID, "0"),
        )
        .expect("inspect json");
        let marker = temp.path().join("started.marker");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  if [ -f \"{}\" ]; then\n    cat \"{}\"\n  else\n    cat \"{}\"\n  fi\n  exit 0\nfi\nif [ \"$1\" = \"start\" ]; then\n  touch \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            ps_file.display(),
            marker.display(),
            running_file.display(),
            stopped_file.display(),
            marker.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        ensure_execution_environment(
            &backend,
            &support::enabled_profile(&support::state_dir(temp.path())),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("restart with a writable state dir must succeed");

        let logged = std::fs::read_to_string(&log).expect("read log");
        let lines: Vec<&str> = logged.lines().collect();
        let start_index = lines
            .iter()
            .position(|line| line.starts_with("start "))
            .expect("start issued");
        let exec_index = lines
            .iter()
            .position(|line| line.starts_with("exec "))
            .expect("probe issued");
        assert!(exec_index > start_index, "probe must follow the restart");
    }

    #[tokio::test]
    async fn failing_probe_refuses_running_without_forced_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-fail.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, state) = creation_backend(temp.path(), &log, 0, "fail");

        let error = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await
        .expect_err("a failed probe must not acknowledge running")
        .to_string();
        assert!(error.contains("not writable"), "got: {error}");
        assert!(!error.contains("read-only"), "probe output must stay out");

        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged
                .lines()
                .any(|line| line.starts_with("stop ") || line.starts_with("rm ")),
            "a probe failure must not stop or remove anything, got: {logged}"
        );
        assert!(
            !logged.contains("volume rm"),
            "a probe failure must not touch the durable volume"
        );
    }

    #[tokio::test]
    async fn hanging_probe_consumes_the_aggregate_deadline_and_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-hang.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, state) = creation_backend(temp.path(), &log, 0, "hang");
        let mut profile = support::enabled_profile(&state);
        profile.operation_deadline = Duration::from_millis(600);

        let started = std::time::Instant::now();
        let error = ensure_execution_environment(
            &backend,
            &profile,
            EnsureExecutionEnvironmentRequest {
                owner: support::env_owner(0),
            },
        )
        .await
        .expect_err("a hanging probe must not acknowledge running")
        .to_string();
        let elapsed = started.elapsed();

        assert!(
            error.contains("deadline"),
            "a hung probe is reported as an expiry, got: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "the probe must be bounded by the aggregate deadline, took {elapsed:?}"
        );
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged
                .lines()
                .any(|line| line.starts_with("stop ") || line.starts_with("rm ")),
            "a probe timeout is not a stop proof, got: {logged}"
        );
    }

    #[tokio::test]
    async fn expired_aggregate_deadline_refuses_before_the_probe_executes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-expired.log");
        std::fs::write(&log, "").expect("seed log");
        let (backend, _state) = creation_backend(temp.path(), &log, 0, "ok");
        let deadline = OperationDeadline::new(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));

        let fixture = support::bare_enabled_profile();
        let process = fixture
            .ensure_process_profile()
            .expect("fixture profile resolves");
        let error = confirm_state_writable(&backend, "df-env-abc-0", &process, &deadline)
            .await
            .expect_err("an expired aggregate budget must refuse the probe")
            .to_string();
        assert!(error.contains("deadline exceeded"));
        assert!(
            std::fs::read_to_string(&log).expect("read log").is_empty(),
            "an expired budget must not execute anything"
        );
    }

    #[tokio::test]
    async fn stop_needs_no_command_or_workdir_and_never_probes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("probe-stop.log");
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
        // A profile that cannot create anything at all: no command, no
        // working directory, no ownership.
        let mut profile = support::disabled_profile(&support::state_dir(temp.path()));
        profile.command = TrustedCommand::Unset;
        profile.state_workdir = TrustedStateWorkdir::Unset;
        profile.volume_uid = None;
        profile.volume_gid = None;

        let ack = stop_execution_environment(
            &backend,
            &profile,
            StopExecutionEnvironmentRequest { owner },
        )
        .await
        .expect("stop must stay operable without creation inputs");
        assert!(ack.confirmed);
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged.lines().any(|line| line.starts_with("exec ")),
            "stop never probes, got: {logged}"
        );
    }
}
