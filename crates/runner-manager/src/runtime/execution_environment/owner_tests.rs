//! Owner-verification tests (issue 740 P1, durable barrier in 742 P1,
//! `#[cfg(test)]`).
//!
//! Every success acknowledgment and destructive command requires matching
//! owner/environment/generation labels: foreign or malformed labels refuse
//! before stop/rm and never record a durable stop barrier, and reused or
//! created containers cannot acknowledge success. Behaviour-level lock
//! serialization across generations is proven with deterministic
//! `try_lock`, not pointer identity. No live daemon.

#[cfg(test)]
mod owner_tests {
    use std::path::Path;
    use std::sync::Arc;

    use runner_protocol::{EnsureExecutionEnvironmentRequest, StopExecutionEnvironmentRequest};

    use super::super::{
        ExecutionEnvironmentProfile, ensure::ensure_execution_environment,
        stop::stop_execution_environment, test_support as support,
    };

    fn enabled(state_root: &Path) -> ExecutionEnvironmentProfile {
        support::enabled_profile(state_root)
    }

    fn destructive_calls(log: &str) -> Vec<String> {
        log.lines()
            .filter(|line| line.starts_with("stop ") || line.starts_with("rm "))
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn stop_preflight_mismatched_labels_emit_no_destructive_args() {
        let mismatches = [
            (
                "owner",
                "execution_environment:foreign:0",
                support::ENV_ID,
                "0",
            ),
            (
                "environment",
                "execution_environment:123e4567-e89b-12d3-a456-426614174000:0",
                "00000000-0000-0000-0000-000000000000",
                "0",
            ),
            (
                "generation",
                "execution_environment:123e4567-e89b-12d3-a456-426614174000:0",
                support::ENV_ID,
                "9",
            ),
        ];
        for (label, owner_key, env_id, generation) in mismatches {
            let temp = tempfile::tempdir().expect("tempdir");
            let state = support::state_dir(temp.path());
            let log = temp.path().join(format!("mismatch-{label}.log"));
            std::fs::write(&log, "").expect("seed log");
            let inspect_file = temp.path().join("inspect.json");
            std::fs::write(
                &inspect_file,
                support::inspect_json("running", owner_key, env_id, generation),
            )
            .expect("inspect json");
            let script = format!(
                "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
                log.display(),
                inspect_file.display()
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
            assert!(result.is_err(), "{label} mismatch must refuse stop");
            assert!(
                result.unwrap_err().to_string().contains("do not match"),
                "{label} mismatch must name the label guard"
            );
            let logged = std::fs::read_to_string(&log).expect("read log");
            assert!(
                destructive_calls(&logged).is_empty(),
                "{label} mismatch must not run stop/rm, got: {logged}"
            );
            assert!(
                support::read_barrier(&state).is_none(),
                "{label} mismatch must not record a durable stop barrier"
            );
        }
    }

    #[tokio::test]
    async fn ensure_reused_mismatched_labels_cannot_acknowledge_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let container = owner.container_name();
        let log = temp.path().join("reuse-mismatch.log");
        std::fs::write(&log, "").expect("seed log");
        let ps_file = temp.path().join("ps.txt");
        std::fs::write(&ps_file, format!("{container}\n")).expect("ps");
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json(
                "running",
                "execution_environment:foreign:0",
                support::ENV_ID,
                "0",
            ),
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

        let result = ensure_execution_environment(
            &backend,
            &enabled(&state),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await;
        assert!(
            result.is_err(),
            "a reused container with foreign labels must not acknowledge success"
        );
        assert!(result.unwrap_err().to_string().contains("do not match"));
        let logged = std::fs::read_to_string(&log).expect("read log");
        assert!(
            !logged.lines().any(|l| l.starts_with("run ")),
            "mismatched reuse must not create, got: {logged}"
        );
    }

    #[tokio::test]
    async fn ensure_created_mismatched_labels_cannot_acknowledge_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let owner = support::env_owner(0);
        let log = temp.path().join("create-mismatch.log");
        std::fs::write(&log, "").expect("seed log");
        let inspect_file = temp.path().join("inspect.json");
        std::fs::write(
            &inspect_file,
            support::inspect_json(
                "running",
                "execution_environment:foreign:0",
                support::ENV_ID,
                "0",
            ),
        )
        .expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            inspect_file.display()
        );
        let fake = support::write_executable(temp.path(), "docker-fake", &script);
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let result = ensure_execution_environment(
            &backend,
            &enabled(&state),
            EnsureExecutionEnvironmentRequest { owner },
        )
        .await;
        assert!(
            result.is_err(),
            "a created container with foreign labels must not acknowledge success"
        );
        assert!(result.unwrap_err().to_string().contains("do not match"));
    }

    #[tokio::test]
    async fn concurrent_generations_serialize_at_environment_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("lock-behaviour.log");
        std::fs::write(&log, "").expect("seed log");
        let fake = support::write_simple_fake(temp.path(), "docker-ok", &log, "ok");
        let backend = support::backend_with_cli(&fake.to_string_lossy());

        let generation0 = backend.runner_lock(&support::env_owner(0));
        let generation1 = backend.runner_lock(&support::env_owner(1));
        assert!(
            Arc::ptr_eq(&generation0, &generation1),
            "one environment must share one lock"
        );

        // Behaviour, not pointer identity: while one generation holds the
        // environment lock, another generation cannot acquire it.
        let guard = generation0
            .try_lock()
            .expect("environment lock starts free");
        assert!(
            generation1.try_lock().is_err(),
            "another generation must serialize behind the environment lock"
        );
        drop(guard);
        assert!(
            generation1.try_lock().is_ok(),
            "releasing the holder must free the environment lock"
        );
    }
}
