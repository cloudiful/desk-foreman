use runner_protocol::{
    EnsureExecutionEnvironmentAck, NativeTaskRuntimeIdentity, RunnerJobResult, RunnerOwner,
    StartNativeTaskResult, StopNativeTaskResult,
};
use sqlx::Transaction;
use uuid::Uuid;

use super::queries::{find_job, seed_identity};
use crate::db::execution_environment_jobs::{
    ExecutionEnvironmentJob, ExecutionEnvironmentJobResult,
};

pub(super) const ACCEPTED_STOP_STATES: [&str; 3] = ["absent", "exited", "dead"];
pub(super) const START_FAILURE: &str = "assigned manager could not confirm native task start";
pub(super) const STOP_FAILURE: &str = "assigned manager could not confirm native task quiescence";

pub(super) struct Resolution {
    pub(super) state: &'static str,
    pub(super) result: ExecutionEnvironmentJobResult,
    pub(super) error_summary: Option<&'static str>,
    pub(super) successful_start: bool,
    pub(super) successful_stop: bool,
}

pub(super) async fn runtime_identity(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task: &crate::db::execution_environments::NativeTask,
    require_succeeded_preparation: bool,
) -> anyhow::Result<Option<NativeTaskRuntimeIdentity>> {
    let Some(prepare_job_id) = task.prepare_job_id else {
        return Ok(None);
    };
    let Some(prepare) = find_job(tx, application_id, environment_id, prepare_job_id)
        .await
        .map_err(|error| anyhow::anyhow!(error.code()))?
    else {
        return Ok(None);
    };
    let preparation_proved = prepare.state == "succeeded"
        && prepare
            .result
            .as_ref()
            .and_then(|value| value.get("outcome"))
            == Some(&serde_json::Value::String("task_prepared".to_string()));
    let preparation_state_allows_stop = matches!(prepare.state.as_str(), "succeeded" | "uncertain");
    if prepare.job_kind != "prepare_native_task"
        || prepare.native_task_id != Some(task.task_id)
        || prepare.assigned_manager_id != task.assigned_manager_id
        || prepare.target_generation != task.target_generation
        || (require_succeeded_preparation && !preparation_proved)
        || (!require_succeeded_preparation && !preparation_state_allows_stop)
    {
        return Ok(None);
    }
    let Some((manifest_sha256, _, _)) = seed_identity(tx, task.task_id)
        .await
        .map_err(|error| anyhow::anyhow!(error.code()))?
    else {
        return Ok(None);
    };
    Ok(Some(NativeTaskRuntimeIdentity {
        task_id: task.task_id,
        workspace_binding_id: task.workspace_binding_id,
        manifest_sha256,
        preparation_generation: prepare.target_generation,
    }))
}

pub(super) fn resolve(
    job: &ExecutionEnvironmentJob,
    reported: &RunnerJobResult,
    expected_task: &NativeTaskRuntimeIdentity,
) -> Result<Resolution, &'static str> {
    if !reported.ok {
        if !reported
            .error
            .as_deref()
            .is_some_and(|error| !error.trim().is_empty())
        {
            return Err("failed native execution result carried no diagnostic");
        }
        return Ok(if job.job_kind == "start_native_task" {
            Resolution {
                state: "uncertain",
                result: ExecutionEnvironmentJobResult {
                    outcome: "native_task_start_uncertain".to_string(),
                    observed_state: None,
                    confirmed: None,
                    preparation_quiescent: None,
                },
                error_summary: Some(START_FAILURE),
                successful_start: false,
                successful_stop: false,
            }
        } else {
            stop_uncertain(None, Some(false))
        });
    }
    let Some(value) = reported.result.clone() else {
        return Err("successful native execution result carried no payload");
    };
    let expected_owner = RunnerOwner::ExecutionEnvironment {
        environment_id: job.environment_id.to_string(),
        generation: job.target_generation,
    };
    match job.job_kind.as_str() {
        "start_native_task" => {
            let ack: StartNativeTaskResult = serde_json::from_value(value)
                .map_err(|_| "native start acknowledgment could not be decoded")?;
            validate_task_identity(&ack.task, expected_task)?;
            validate_environment_ack(&ack.environment, &expected_owner, "running")?;
            Ok(Resolution {
                state: "succeeded",
                result: ExecutionEnvironmentJobResult {
                    outcome: "native_task_running".to_string(),
                    observed_state: Some(ack.environment.state),
                    confirmed: None,
                    preparation_quiescent: None,
                },
                error_summary: None,
                successful_start: true,
                successful_stop: false,
            })
        }
        "stop" if job.native_task_id.is_some() => {
            let ack: StopNativeTaskResult = serde_json::from_value(value)
                .map_err(|_| "native stop acknowledgment could not be decoded")?;
            validate_task_identity(&ack.task, expected_task)?;
            if ack.environment.owner != expected_owner {
                return Err("native stop owner does not match the assigned task generation");
            }
            if ack.environment.container_name != expected_owner.container_name() {
                return Err("native stop container does not match the assigned task");
            }
            if ack.environment.confirmed
                && !ACCEPTED_STOP_STATES.contains(&ack.environment.observed_state.as_str())
            {
                return Err("native stop reported an unrecognized confirmed state");
            }
            if ack.environment.confirmed && ack.preparation_quiescent {
                Ok(Resolution {
                    state: "succeeded",
                    result: ExecutionEnvironmentJobResult {
                        outcome: "stop_confirmed".to_string(),
                        observed_state: Some(ack.environment.observed_state),
                        confirmed: Some(true),
                        preparation_quiescent: Some(true),
                    },
                    error_summary: None,
                    successful_start: false,
                    successful_stop: true,
                })
            } else {
                Ok(stop_uncertain(
                    Some(ack.environment.observed_state),
                    Some(ack.preparation_quiescent),
                ))
            }
        }
        _ => Err("job is not a native task execution job"),
    }
}

fn validate_task_identity(
    actual: &NativeTaskRuntimeIdentity,
    expected: &NativeTaskRuntimeIdentity,
) -> Result<(), &'static str> {
    if actual == expected {
        Ok(())
    } else {
        Err("native task acknowledgment does not match its stored identity")
    }
}

fn validate_environment_ack(
    ack: &EnsureExecutionEnvironmentAck,
    expected_owner: &RunnerOwner,
    expected_state: &str,
) -> Result<(), &'static str> {
    if ack.owner != *expected_owner {
        return Err("native start owner does not match the assigned task generation");
    }
    if ack.container_name != expected_owner.container_name() {
        return Err("native start container does not match the assigned task");
    }
    if ack.state != expected_state {
        return Err("native start did not confirm a running task");
    }
    Ok(())
}

fn stop_uncertain(
    observed_state: Option<String>,
    preparation_quiescent: Option<bool>,
) -> Resolution {
    Resolution {
        state: "uncertain",
        result: ExecutionEnvironmentJobResult {
            outcome: "stop_unconfirmed".to_string(),
            observed_state,
            confirmed: Some(false),
            preparation_quiescent,
        },
        error_summary: Some(STOP_FAILURE),
        successful_start: false,
        successful_stop: false,
    }
}
