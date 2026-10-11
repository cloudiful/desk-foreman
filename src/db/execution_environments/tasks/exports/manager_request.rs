use runner_protocol::ExportNativeTaskRequest;
use sqlx::PgPool;

use super::{identity::request_for_header, proof::lock_matching_stop_proof, queries};
use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

pub(crate) async fn request_for_export_job(
    pool: &PgPool,
    claimed: &ExecutionEnvironmentJob,
) -> anyhow::Result<ExportNativeTaskRequest> {
    let task_id = claimed
        .native_task_id
        .ok_or_else(|| anyhow::anyhow!("native task export has no task link"))?;
    let mut tx = pool.begin().await?;
    let (environment, task, binding) = queries::lock_context(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        task_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?;
    let job = queries::lock_job(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        claimed.job_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?
    .ok_or_else(|| anyhow::anyhow!("native task export job disappeared"))?;
    let header =
        queries::find_export_by_job(&mut tx, job.job_id, job.environment_id, job.application_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("native task export metadata is absent"))?;
    if job.job_kind != "export_native_task"
        || job.state != "claimed"
        || job.native_task_id != Some(task_id)
        || job.assigned_manager_id != claimed.assigned_manager_id
        || header.job_id != job.job_id
        || header.assigned_manager_id != job.assigned_manager_id
        || header.target_generation != job.target_generation
        || binding.workspace_binding_id != task.workspace_binding_id
        || !queries::claim_is_live(
            &mut tx,
            job.job_id,
            job.application_id,
            job.environment_id,
            task_id,
            job.assigned_manager_id,
        )
        .await?
    {
        anyhow::bail!("native task export job is no longer dispatchable");
    }
    let _proof = lock_matching_stop_proof(
        &mut tx,
        job.application_id,
        &environment,
        &task,
        header.stop_job_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?;
    let request = request_for_header(&mut tx, &environment, &task, &header)
        .await
        .map_err(|error| anyhow::anyhow!(error.code()))?;
    tx.commit().await?;
    Ok(request)
}
