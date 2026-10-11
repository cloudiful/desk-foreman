//! Unit tests for ack resolution (issue 742 P2, `#[cfg(test)]`).
//!
//! Proves the exact owner/generation/container/state invariants without a
//! database: consistent success, uncertain non-success, and every
//! inconsistent shape.

#[cfg(test)]
mod resolution_tests {
    use chrono::Utc;
    use runner_protocol::{
        EnsureExecutionEnvironmentAck, RunnerJobResult, RunnerOwner, StopExecutionEnvironmentAck,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::super::ExecutionEnvironmentJob;
    use super::super::dispatch::encode_environment_job_wire_id;
    use super::super::resolution::{
        EnvironmentJobResolution, environment_job_owner, environment_job_wire_kind,
        resolve_environment_job_result,
    };

    const ENV_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

    fn row(kind: &str, generation: i64) -> ExecutionEnvironmentJob {
        ExecutionEnvironmentJob {
            job_id: Uuid::parse_str("00000000-0000-0000-0000-0000000000aa").unwrap(),
            token: "tok".to_string(),
            environment_id: Uuid::parse_str(ENV_ID).unwrap(),
            application_id: 1,
            assigned_manager_id: 7,
            job_kind: kind.to_string(),
            target_generation: generation,
            admission_generation: None,
            native_task_id: None,
            state: "claimed".to_string(),
            claimed_at: Some(Utc::now()),
            deadline_at: Utc::now() + chrono::Duration::seconds(60),
            terminal_at: None,
            result: None,
            error_summary: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn owner(generation: i64) -> RunnerOwner {
        RunnerOwner::ExecutionEnvironment {
            environment_id: ENV_ID.to_string(),
            generation,
        }
    }

    fn ok_result(value: serde_json::Value) -> RunnerJobResult {
        RunnerJobResult {
            job_id: encode_environment_job_wire_id(row("ensure", 0).job_id),
            ok: true,
            result: Some(value),
            error: None,
        }
    }

    #[test]
    fn wire_kind_maps_checked_kinds() {
        assert!(environment_job_wire_kind(&row("ensure", 0)).is_ok());
        assert!(environment_job_wire_kind(&row("stop", 0)).is_ok());
        assert!(environment_job_wire_kind(&row("restart", 0)).is_err());
    }

    #[test]
    fn ensure_ack_requires_matching_owner_and_running() {
        let running = ok_result(json!(EnsureExecutionEnvironmentAck {
            owner: owner(2),
            container_name: owner(2).container_name(),
            state: "running".into(),
        }));
        match resolve_environment_job_result(&row("ensure", 2), &running) {
            EnvironmentJobResolution::Succeeded(result) => {
                assert_eq!(result.outcome, "ensure_running");
                assert_eq!(result.observed_state.as_deref(), Some("running"));
            }
            other => panic!("expected success, got {other:?}"),
        }

        let wrong_generation = ok_result(json!(EnsureExecutionEnvironmentAck {
            owner: owner(3),
            container_name: owner(3).container_name(),
            state: "running".into(),
        }));
        assert!(matches!(
            resolve_environment_job_result(&row("ensure", 2), &wrong_generation),
            EnvironmentJobResolution::Inconsistent(_)
        ));

        let wrong_name = ok_result(json!(EnsureExecutionEnvironmentAck {
            owner: owner(2),
            container_name: "someone-elses-container".into(),
            state: "running".into(),
        }));
        assert!(matches!(
            resolve_environment_job_result(&row("ensure", 2), &wrong_name),
            EnvironmentJobResolution::Inconsistent(_)
        ));

        let not_running = ok_result(json!(EnsureExecutionEnvironmentAck {
            owner: owner(2),
            container_name: owner(2).container_name(),
            state: "exited".into(),
        }));
        assert!(matches!(
            resolve_environment_job_result(&row("ensure", 2), &not_running),
            EnvironmentJobResolution::Inconsistent(_)
        ));
    }

    #[test]
    fn stop_ack_accepts_only_positive_confirmed_quiescence() {
        for state in ["absent", "exited", "dead"] {
            let ack = ok_result(json!(StopExecutionEnvironmentAck {
                owner: owner(0),
                container_name: owner(0).container_name(),
                confirmed: true,
                observed_state: state.into(),
            }));
            match resolve_environment_job_result(&row("stop", 0), &ack) {
                EnvironmentJobResolution::Succeeded(result) => {
                    assert_eq!(result.outcome, "stop_confirmed");
                    assert_eq!(result.confirmed, Some(true));
                }
                other => panic!("{state} must confirm, got {other:?}"),
            }
        }

        // `removed` is explicitly not an accepted confirmation.
        let removed = ok_result(json!(StopExecutionEnvironmentAck {
            owner: owner(0),
            container_name: owner(0).container_name(),
            confirmed: true,
            observed_state: "removed".into(),
        }));
        assert!(matches!(
            resolve_environment_job_result(&row("stop", 0), &removed),
            EnvironmentJobResolution::Inconsistent(_)
        ));

        let arbitrary = ok_result(json!(StopExecutionEnvironmentAck {
            owner: owner(0),
            container_name: owner(0).container_name(),
            confirmed: true,
            observed_state: "whatever".into(),
        }));
        assert!(matches!(
            resolve_environment_job_result(&row("stop", 0), &arbitrary),
            EnvironmentJobResolution::Inconsistent(_)
        ));

        let unconfirmed = ok_result(json!(StopExecutionEnvironmentAck {
            owner: owner(0),
            container_name: owner(0).container_name(),
            confirmed: false,
            observed_state: "running".into(),
        }));
        match resolve_environment_job_result(&row("stop", 0), &unconfirmed) {
            EnvironmentJobResolution::Uncertain(result, diagnostic) => {
                assert_eq!(result.outcome, "stop_unconfirmed");
                assert_eq!(result.confirmed, Some(false));
                assert!(!diagnostic.is_empty());
            }
            other => panic!("expected uncertain, got {other:?}"),
        }
    }

    #[test]
    fn failure_and_malformed_results_are_classified() {
        let diagnosed = RunnerJobResult {
            job_id: encode_environment_job_wire_id(row("ensure", 0).job_id),
            ok: false,
            result: None,
            error: Some("boom".to_string()),
        };
        match resolve_environment_job_result(&row("ensure", 0), &diagnosed) {
            EnvironmentJobResolution::Uncertain(result, diagnostic) => {
                assert_eq!(result.outcome, "ensure_uncertain");
                assert!(result.confirmed.is_none());
                assert!(diagnostic.contains("failed"));
            }
            other => panic!("expected uncertain, got {other:?}"),
        }

        let undiagnosed = RunnerJobResult {
            job_id: encode_environment_job_wire_id(row("ensure", 0).job_id),
            ok: false,
            result: None,
            error: None,
        };
        assert!(matches!(
            resolve_environment_job_result(&row("ensure", 0), &undiagnosed),
            EnvironmentJobResolution::Inconsistent(_)
        ));

        let no_payload = RunnerJobResult {
            job_id: encode_environment_job_wire_id(row("ensure", 0).job_id),
            ok: true,
            result: None,
            error: None,
        };
        assert!(matches!(
            resolve_environment_job_result(&row("ensure", 0), &no_payload),
            EnvironmentJobResolution::Inconsistent(_)
        ));

        let garbage = ok_result(json!({"not": "an ack"}));
        assert!(matches!(
            resolve_environment_job_result(&row("ensure", 0), &garbage),
            EnvironmentJobResolution::Inconsistent(_)
        ));
    }

    #[test]
    fn owner_derivation_matches_the_row() {
        assert_eq!(environment_job_owner(&row("ensure", 5)), owner(5));
    }
}
