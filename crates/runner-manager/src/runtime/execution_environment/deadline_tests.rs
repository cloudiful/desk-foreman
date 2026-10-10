//! Aggregate operation-deadline tests (issue 740 P1, durable barrier in
//! 742 P1, `#[cfg(test)]`).
//!
//! Deterministic fake-CLI hang fakes prove that a stall at the lock, ps,
//! volume, run, start or final inspect ends within the configured
//! aggregate deadline with an error, never a success, and that the bounded
//! command runner kills and reaps the spawned CLI. No live daemon.

#[cfg(test)]
mod deadline_tests {
    use std::path::Path;
    use std::time::Duration;

    use runner_protocol::{EnsureExecutionEnvironmentRequest, StopExecutionEnvironmentRequest};

    use super::super::{
        ExecutionEnvironmentProfile, ensure::ensure_execution_environment,
        stop::stop_execution_environment, test_support as support,
    };

    fn enabled_with_deadline(state_root: &Path, ms: u64) -> ExecutionEnvironmentProfile {
        let mut profile = support::enabled_profile(state_root);
        profile.operation_deadline = Duration::from_millis(ms);
        profile
    }

    fn ensure_req(generation: i64) -> EnsureExecutionEnvironmentRequest {
        EnsureExecutionEnvironmentRequest {
            owner: support::env_owner(generation),
        }
    }

    fn stop_req(generation: i64) -> StopExecutionEnvironmentRequest {
        StopExecutionEnvironmentRequest {
            owner: support::env_owner(generation),
        }
    }

    #[tokio::test]
    async fn zero_deadline_errors_before_any_docker_call() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("zero.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let mut profile = ExecutionEnvironmentProfile::test_enabled();
        profile.operation_deadline = Duration::ZERO;

        let ensure = ensure_execution_environment(&backend, &profile, ensure_req(0)).await;
        assert!(ensure.is_err());
        assert!(ensure.unwrap_err().to_string().contains("positive"));

        let stop = stop_execution_environment(&backend, &profile, stop_req(0)).await;
        assert!(stop.is_err());
        assert!(stop.unwrap_err().to_string().contains("positive"));

        assert!(
            std::fs::read_to_string(&log).expect("read log").is_empty(),
            "a zero deadline must not touch docker"
        );
    }

    #[tokio::test]
    async fn ensure_lock_stall_is_bounded_and_reaches_no_docker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("lock.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let lock = backend.environment_runner_lock(support::ENV_ID);
        let guard = lock.lock().await;

        let result = ensure_execution_environment(
            &backend,
            &enabled_with_deadline(&state, 120),
            ensure_req(0),
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("timed out acquiring"),
            "lock stall must be reported as a deadline failure"
        );
        assert!(std::fs::read_to_string(&log).expect("read log").is_empty());
        drop(guard);
    }

    #[tokio::test]
    async fn stop_lock_stall_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("lock-stop.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let lock = backend.environment_runner_lock(support::ENV_ID);
        let guard = lock.lock().await;

        let result =
            stop_execution_environment(&backend, &enabled_with_deadline(&state, 120), stop_req(0))
                .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("timed out acquiring")
        );
        assert!(std::fs::read_to_string(&log).expect("read log").is_empty());
        drop(guard);
    }

    #[tokio::test]
    async fn ensure_volume_stall_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("volume.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_hang_fake(temp.path(), "docker-hang-volume", &log, "volume");
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let result = ensure_execution_environment(
            &backend,
            &enabled_with_deadline(&state, 500),
            ensure_req(0),
        )
        .await;
        let error = result.expect_err("volume stall must fail");
        assert!(error.to_string().contains("timed out"), "got: {error}");
    }

    #[tokio::test]
    async fn ensure_ps_stall_is_bounded_and_reaps_the_cli() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("ps.log");
        std::fs::write(&log, "").expect("seed log");
        let pid_file = temp.path().join("ps.pid");
        let fake =
            support::write_hang_fake_with_pid(temp.path(), "docker-hang-ps", &log, "ps", &pid_file);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let result = ensure_execution_environment(
            &backend,
            &enabled_with_deadline(&state, 500),
            ensure_req(0),
        )
        .await;
        let error = result.expect_err("ps stall must fail");
        assert!(error.to_string().contains("timed out"), "got: {error}");

        // The bounded runner killed and reaped the CLI before returning.
        let raw = std::fs::read_to_string(&pid_file).expect("pid file");
        let pid: i32 = raw.trim().parse().expect("pid int");
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "spawned docker CLI must be reaped, pid {pid} still alive"
        );
    }

    #[tokio::test]
    async fn ensure_run_stall_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("run.log");
        std::fs::write(&log, "").expect("seed log");
        // `volume` and `ps` exit 0 (empty ps), so the `run` step is reached.
        let fake = support::write_hang_fake(temp.path(), "docker-hang-run", &log, "run");
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let result = ensure_execution_environment(
            &backend,
            &enabled_with_deadline(&state, 500),
            ensure_req(0),
        )
        .await;
        let error = result.expect_err("run stall must fail");
        assert!(error.to_string().contains("timed out"), "got: {error}");
    }

    #[tokio::test]
    async fn ensure_start_stall_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let container = owner.container_name();
        let log = temp.path().join("start.log");
        std::fs::write(&log, "").expect("seed log");
        let stopped = support::inspect_json("exited", &owner.stable_key(), support::ENV_ID, "0");
        let stopped_file = temp.path().join("stopped.json");
        std::fs::write(&stopped_file, stopped).expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  echo \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"start\" ]; then\n  sleep 30\n  exit 0\nfi\nexit 0\n",
            log.display(),
            container,
            stopped_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-hang-start", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let result = ensure_execution_environment(
            &backend,
            &enabled_with_deadline(&state, 600),
            ensure_req(0),
        )
        .await;
        let error = result.expect_err("start stall must fail");
        assert!(error.to_string().contains("timed out"), "got: {error}");
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged.lines().any(|l| l.starts_with("run ")),
            "a stalled start must not fall through to creation, got: {logged}"
        );
    }

    #[tokio::test]
    async fn stop_final_inspect_stall_is_bounded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let log = temp.path().join("final-inspect.log");
        std::fs::write(&log, "").expect("seed log");
        let running = support::inspect_json("running", &owner.stable_key(), support::ENV_ID, "0");
        let running_file = temp.path().join("running.json");
        std::fs::write(&running_file, running).expect("inspect json");
        let marker = temp.path().join("stopped.marker");
        // First inspect reports a matching running owner; once stop runs,
        // the final inspect stalls so confirmation must fail closed.
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  if [ -f \"{}\" ]; then\n    sleep 30\n    exit 0\n  fi\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"stop\" ]; then\n  touch \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            marker.display(),
            running_file.display(),
            marker.display()
        );
        let fake = support::write_executable(temp.path(), "docker-hang-final", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let result =
            stop_execution_environment(&backend, &enabled_with_deadline(&state, 500), stop_req(0))
                .await;
        let error = result.expect_err("final inspect stall must not confirm");
        assert!(error.to_string().contains("timed out"), "got: {error}");
    }
}
