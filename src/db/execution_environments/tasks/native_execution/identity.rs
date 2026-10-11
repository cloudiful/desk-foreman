use runner_protocol::{
    NativeTaskRuntimeIdentity, RunnerOwner, StartNativeTaskRequest, StopNativeTaskRequest,
};
use serde_json::Value;

use super::queries::{find_job, lock_context, lock_job, seed_identity};
use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

/// Build the server-owned typed request for a claimed native job.
pub(crate) async fn request_for_native_job(
    pool: &sqlx::PgPool,
    claimed: &ExecutionEnvironmentJob,
) -> anyhow::Result<Value> {
    let task_id = claimed
        .native_task_id
        .ok_or_else(|| anyhow::anyhow!("native execution job has no task link"))?;
    let mut tx = pool.begin().await?;
    let (environment, task, _binding) = lock_context(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        task_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?;
    let job = lock_job(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        claimed.job_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?
    .ok_or_else(|| anyhow::anyhow!("native execution job disappeared"))?;

    if job.state != "claimed"
        || job.deadline_at <= chrono::Utc::now()
        || job.native_task_id != Some(task_id)
        || job.assigned_manager_id != claimed.assigned_manager_id
        || environment.assigned_manager_id != Some(job.assigned_manager_id)
        || environment.active_native_task_id != Some(task_id)
    {
        anyhow::bail!("native execution job is no longer dispatchable");
    }

    let prepare_job_id = task
        .prepare_job_id
        .ok_or_else(|| anyhow::anyhow!("native task has no preparation link"))?;
    let prepare = find_job(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        prepare_job_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?
    .ok_or_else(|| anyhow::anyhow!("native task preparation is absent"))?;
    let preparation_proved = prepare.state == "succeeded"
        && prepare
            .result
            .as_ref()
            .and_then(|value| value.get("outcome"))
            == Some(&Value::String("task_prepared".to_string()));
    let preparation_state_allows_stop = matches!(prepare.state.as_str(), "succeeded" | "uncertain");
    if prepare.job_kind != "prepare_native_task"
        || prepare.native_task_id != Some(task_id)
        || prepare.assigned_manager_id != task.assigned_manager_id
        || prepare.target_generation != task.target_generation
        || (job.job_kind == "start_native_task" && !preparation_proved)
        || (job.job_kind == "stop" && !preparation_state_allows_stop)
    {
        anyhow::bail!("native task preparation identity is no longer valid");
    }
    let Some((manifest_sha256, _, _)) = seed_identity(&mut tx, task_id)
        .await
        .map_err(|error| anyhow::anyhow!(error.code()))?
    else {
        anyhow::bail!("native task seed is absent");
    };
    let identity = NativeTaskRuntimeIdentity {
        task_id,
        workspace_binding_id: task.workspace_binding_id,
        manifest_sha256,
        preparation_generation: prepare.target_generation,
    };
    let owner = RunnerOwner::ExecutionEnvironment {
        environment_id: environment.environment_id.to_string(),
        generation: job.target_generation,
    };

    let payload = match job.job_kind.as_str() {
        "start_native_task" => {
            if task.state != "starting"
                || task.start_job_id != Some(job.job_id)
                || task.target_generation != job.target_generation
                || task.container_generation != Some(job.target_generation)
                || environment.state != "starting"
                || environment.container_generation != Some(job.target_generation)
                || environment.generation != job.target_generation
            {
                anyhow::bail!("native start is no longer current");
            }
            serde_json::to_value(StartNativeTaskRequest {
                owner,
                task: identity,
            })?
        }
        "stop" if job.native_task_id.is_some() => {
            if task.state != "stopping"
                || task.stop_job_id != Some(job.job_id)
                || task.target_generation != job.target_generation
                || task.container_generation != Some(job.target_generation)
                || environment.container_generation != Some(job.target_generation)
                || environment.generation <= job.target_generation
            {
                anyhow::bail!("native stop is no longer current");
            }
            serde_json::to_value(StopNativeTaskRequest {
                owner,
                task: identity,
            })?
        }
        _ => anyhow::bail!("job is not a native task execution job"),
    };
    tx.commit().await?;
    Ok(payload)
}
