//! Pure result evaluation for durable environment jobs (issue 742 P2).
//!
//! Given one durable row and one manager-reported result, decide whether the
//! ack is a consistent terminal success, a valid non-success outcome that must
//! block as `uncertain`, or an inconsistent ack that mutates nothing. No store
//! access happens here, so the owner/generation/container/state invariants are
//! unit-testable in isolation.

use runner_protocol::{
    EnsureExecutionEnvironmentAck, RunnerJobResult, RunnerOwner, StopExecutionEnvironmentAck,
    execution_environment as protocol,
};

use super::types::{ExecutionEnvironmentJob, ExecutionEnvironmentJobResult};

/// Wire job kind the manager executes for an `ensure` record.
pub(crate) const ENSURE_WIRE_KIND: &str = "ensure_execution_environment";
/// Wire job kind the manager executes for a `stop` record.
pub(crate) const STOP_WIRE_KIND: &str = "stop_execution_environment";
/// Wire job kind the manager executes for a native-task preparation record.
pub(crate) const PREPARE_WIRE_KIND: &str = "prepare_native_task";

/// States a positive stop confirmation may report. `removed` is deliberately
/// excluded: it is not a confirmed quiescence of the requested generation.
const ACCEPTED_CONFIRMED_STOP_STATES: [&str; 3] = ["absent", "exited", "dead"];

/// Fixed, redacted diagnostic persisted when the assigned manager reports a
/// failed environment operation. The manager's own error text is never echoed,
/// so no secret can reach the record.
const UNCERTAIN_OPERATION_DIAGNOSTIC: &str =
    "assigned runner manager reported a failed execution environment operation";
/// Fixed, redacted diagnostic persisted when a stop cannot be confirmed.
const UNCERTAIN_STOP_DIAGNOSTIC: &str =
    "assigned runner manager could not confirm the execution environment stop";

/// The exact environment owner a durable row targets.
pub(crate) fn environment_job_owner(row: &ExecutionEnvironmentJob) -> RunnerOwner {
    RunnerOwner::ExecutionEnvironment {
        environment_id: row.environment_id.to_string(),
        generation: row.target_generation,
    }
}

/// The wire job kind the manager must execute for this row.
pub(crate) fn environment_job_wire_kind(
    row: &ExecutionEnvironmentJob,
) -> anyhow::Result<&'static str> {
    match row.job_kind.as_str() {
        "ensure" => Ok(ENSURE_WIRE_KIND),
        "stop" => Ok(STOP_WIRE_KIND),
        "prepare_native_task" => Ok(PREPARE_WIRE_KIND),
        other => anyhow::bail!("durable environment job has an unknown kind: {other}"),
    }
}

/// Evaluation of one manager-reported result against its durable row.
#[derive(Debug)]
pub(crate) enum EnvironmentJobResolution {
    /// Consistent terminal success; persist as `succeeded`.
    Succeeded(ExecutionEnvironmentJobResult),
    /// A valid non-success transport outcome; persist as `uncertain` with a
    /// fixed redacted diagnostic.
    Uncertain(ExecutionEnvironmentJobResult, &'static str),
    /// Malformed or inconsistent ack; mutate nothing.
    Inconsistent(&'static str),
}

/// Pure evaluation of one manager result against its durable row.
pub(crate) fn resolve_environment_job_result(
    row: &ExecutionEnvironmentJob,
    result: &RunnerJobResult,
) -> EnvironmentJobResolution {
    if !result.ok {
        let diagnosed = result
            .error
            .as_deref()
            .is_some_and(|error| !error.trim().is_empty());
        if diagnosed {
            return EnvironmentJobResolution::Uncertain(
                uncertain_result_for_kind(&row.job_kind),
                UNCERTAIN_OPERATION_DIAGNOSTIC,
            );
        }
        return EnvironmentJobResolution::Inconsistent("failed result carried no diagnostic");
    }

    let Some(value) = result.result.clone() else {
        return EnvironmentJobResolution::Inconsistent("successful result carried no payload");
    };
    let expected_owner = environment_job_owner(row);

    match row.job_kind.as_str() {
        "ensure" => match serde_json::from_value::<EnsureExecutionEnvironmentAck>(value) {
            Ok(ack) => {
                if let Err(reason) =
                    owner_ack_matches(&ack.owner, &ack.container_name, &expected_owner)
                {
                    return EnvironmentJobResolution::Inconsistent(reason);
                }
                if ack.state == protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE {
                    EnvironmentJobResolution::Succeeded(ExecutionEnvironmentJobResult {
                        outcome: "ensure_running".to_string(),
                        observed_state: Some(ack.state),
                        confirmed: None,
                    })
                } else {
                    EnvironmentJobResolution::Inconsistent(
                        "ensure ack did not confirm a running environment",
                    )
                }
            }
            Err(_) => EnvironmentJobResolution::Inconsistent("ensure ack could not be decoded"),
        },
        "stop" => match serde_json::from_value::<StopExecutionEnvironmentAck>(value) {
            Ok(ack) => {
                if let Err(reason) =
                    owner_ack_matches(&ack.owner, &ack.container_name, &expected_owner)
                {
                    return EnvironmentJobResolution::Inconsistent(reason);
                }
                if ack.confirmed {
                    if ACCEPTED_CONFIRMED_STOP_STATES.contains(&ack.observed_state.as_str()) {
                        EnvironmentJobResolution::Succeeded(
                            ExecutionEnvironmentJobResult::stop_confirmed(&ack.observed_state),
                        )
                    } else {
                        EnvironmentJobResolution::Inconsistent(
                            "stop ack confirmed an unrecognized stopped state",
                        )
                    }
                } else {
                    EnvironmentJobResolution::Uncertain(
                        ExecutionEnvironmentJobResult::stop_unconfirmed(&ack.observed_state),
                        UNCERTAIN_STOP_DIAGNOSTIC,
                    )
                }
            }
            Err(_) => EnvironmentJobResolution::Inconsistent("stop ack could not be decoded"),
        },
        _ => EnvironmentJobResolution::Inconsistent("durable job kind is not dispatchable"),
    }
}

fn owner_ack_matches(
    owner: &RunnerOwner,
    container_name: &str,
    expected: &RunnerOwner,
) -> Result<(), &'static str> {
    if owner != expected {
        return Err("ack owner does not match the assigned environment generation");
    }
    if container_name != expected.container_name().as_str() {
        return Err("ack container name does not match the assigned environment");
    }
    Ok(())
}

fn uncertain_result_for_kind(job_kind: &str) -> ExecutionEnvironmentJobResult {
    let outcome = match job_kind {
        "stop" => "stop_uncertain",
        "prepare_native_task" => "task_prepared_uncertain",
        _ => "ensure_uncertain",
    };
    ExecutionEnvironmentJobResult {
        outcome: outcome.to_string(),
        observed_state: None,
        confirmed: None,
    }
}
