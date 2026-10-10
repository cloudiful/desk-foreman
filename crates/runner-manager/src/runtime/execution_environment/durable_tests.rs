//! Durable barrier + reservation integration tests (issue 742 P1,
//! `#[cfg(test)]`).
//!
//! Drives real ensure/stop through the fake-Docker harness to prove the
//! durable generation barrier survives a reopened store and stops a late or
//! lower ensure from reaching Docker, and that a missing root, a
//! cross-process reservation, or corrupt state fails closed before any
//! Docker call. No live daemon.

#[cfg(test)]
mod durable_tests {
    use runner_protocol::{EnsureExecutionEnvironmentRequest, StopExecutionEnvironmentRequest};

    use super::super::{
        ensure::ensure_execution_environment, stop::stop_execution_environment,
        test_support as support,
    };

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
    async fn stopped_generation_survives_reopen_and_blocks_late_ensure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(1);
        let log = temp.path().join("durable-stop.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_stop_fake(
            temp.path(),
            "docker-durable-stop",
            &log,
            &owner.stable_key(),
            1,
        );
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let ack =
            stop_execution_environment(&backend, &support::enabled_profile(&state), stop_req(1))
                .await
                .expect("stop must confirm");
        assert!(ack.confirmed);
        let barrier = support::read_barrier(&state).expect("stop barrier");
        assert_eq!(barrier["stopped_generation"], serde_json::json!(1));

        // A fresh ensure opens a new store (reopen) and must refuse the same
        // and a lower generation before any Docker command.
        std::fs::write(&log, "").expect("clear log");
        let same = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            ensure_req(1),
        )
        .await;
        assert!(
            same.is_err(),
            "a durably stopped generation must not restart"
        );
        assert!(
            same.unwrap_err().to_string().contains("durable-stopped"),
            "refusal must name the durable barrier"
        );
        let lower = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            ensure_req(0),
        )
        .await;
        assert!(lower.is_err(), "a lower generation must not restart");

        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged
                .lines()
                .any(|l| l.starts_with("run ") || l.starts_with("start ")),
            "late/lower ensure must not reach Docker, got: {logged}"
        );
    }

    #[tokio::test]
    async fn higher_attempted_generation_blocks_a_lower_ensure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(3);
        let log = temp.path().join("durable-attempt.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_creation_fake(
            temp.path(),
            "docker-durable-create",
            &log,
            &owner.stable_key(),
            3,
        );
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        ensure_execution_environment(&backend, &support::enabled_profile(&state), ensure_req(3))
            .await
            .expect("generation 3 ensure must succeed");
        assert_eq!(
            support::read_barrier(&state).expect("barrier")["high_water"],
            serde_json::json!(3)
        );

        std::fs::write(&log, "").expect("clear log");
        let lower = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            ensure_req(1),
        )
        .await;
        assert!(lower.is_err(), "lower generation must be refused");
        assert!(lower.unwrap_err().to_string().contains("high-water"));
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged
                .lines()
                .any(|l| l.starts_with("run ") || l.starts_with("start ")),
            "lower ensure must not reach Docker, got: {logged}"
        );
    }

    #[tokio::test]
    async fn missing_persistent_root_fails_before_docker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("missing-root.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());
        let profile = support::bare_enabled_profile();

        let ensure = ensure_execution_environment(&backend, &profile, ensure_req(0)).await;
        assert!(ensure.is_err());
        assert!(
            ensure
                .unwrap_err()
                .to_string()
                .contains("persistent state directory is not configured")
        );

        let stop = stop_execution_environment(&backend, &profile, stop_req(0)).await;
        assert!(stop.is_err());
        assert!(
            stop.unwrap_err()
                .to_string()
                .contains("persistent state directory is not configured")
        );

        assert!(
            std::fs::read_to_string(&log).expect("read log").is_empty(),
            "a missing persistent root must not touch docker"
        );
    }

    #[tokio::test]
    async fn existing_reservation_blocks_both_paths_without_docker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("reserved.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let lock = support::lock_path(&state);
        std::fs::create_dir_all(lock.parent().expect("lock parent")).expect("state dir");
        std::fs::write(&lock, "manager=other;pid=1;nonce=1").expect("seed reservation");

        let ensure = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            ensure_req(0),
        )
        .await;
        assert!(ensure.is_err());
        assert!(ensure.unwrap_err().to_string().contains("already held"));
        let stop =
            stop_execution_environment(&backend, &support::enabled_profile(&state), stop_req(0))
                .await;
        assert!(stop.is_err());
        assert!(stop.unwrap_err().to_string().contains("already held"));

        assert!(
            std::fs::read_to_string(&log).expect("read log").is_empty(),
            "a held reservation must not touch docker"
        );
    }

    #[tokio::test]
    async fn corrupt_barrier_fails_closed_before_docker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let log = temp.path().join("corrupt.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let barrier = support::barrier_path(&state);
        std::fs::create_dir_all(barrier.parent().expect("barrier parent")).expect("state dir");
        std::fs::write(&barrier, "not-json").expect("seed corrupt barrier");

        let ensure = ensure_execution_environment(
            &backend,
            &support::enabled_profile(&state),
            ensure_req(0),
        )
        .await;
        assert!(ensure.is_err());
        assert!(ensure.unwrap_err().to_string().contains("corrupt"));
        let stop =
            stop_execution_environment(&backend, &support::enabled_profile(&state), stop_req(0))
                .await;
        assert!(stop.is_err());
        assert!(stop.unwrap_err().to_string().contains("corrupt"));

        assert!(
            std::fs::read_to_string(&log).expect("read log").is_empty(),
            "corrupt state must not touch docker"
        );
    }
}
