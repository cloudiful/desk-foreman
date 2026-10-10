//! Environment transition updates for the lifecycle orchestration (issue 742
//! P3).
//!
//! Each helper is a single owner-scoped statement run on the caller's
//! transaction under the environment row lock. They only move the environment
//! between checked lifecycle states or supersede an obsolete ensure row; none
//! of them performs Docker or network work.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

use super::types::LifecycleEnvironment;

/// Bind the manager if unset and record the provision candidate.
pub(crate) async fn provision_update(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    manager_id: i64,
    candidate_generation: i64,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/provision_lifecycle_environment.sql",
        environment_id,
        application_id,
        manager_id,
        candidate_generation,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// First stop: increment the authorization generation once and mark
/// `stopping`.
pub(crate) async fn begin_stop(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/begin_lifecycle_stop.sql",
        environment_id,
        application_id,
        expected_generation,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Recovery stop: mark `stopping` without another generation increment.
pub(crate) async fn resume_stop(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/resume_lifecycle_stop.sql",
        environment_id,
        application_id,
        expected_generation,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Mark a `starting` environment `ready` after a matching ensure success.
pub(crate) async fn mark_ready(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/reconcile_lifecycle_ready.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Record a confirmed stop proof and mark `replaced`.
pub(crate) async fn mark_replaced(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    proof_job_id: Uuid,
    proof_generation: i64,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/reconcile_lifecycle_replaced.sql",
        environment_id,
        application_id,
        proof_job_id,
        proof_generation,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Downgrade a `stopping` environment to `uncertain` after an unconfirmed stop.
pub(crate) async fn mark_uncertain(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/reconcile_lifecycle_uncertain.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Admit a replacement: consume the proof and advance the candidate.
pub(crate) async fn replace_update(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    new_candidate: i64,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/replace_lifecycle_environment.sql",
        environment_id,
        application_id,
        new_candidate,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Supersede matching obsolete ensure rows with an explicit stop-proof result.
pub(crate) async fn supersede_ensure_jobs(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    manager_id: i64,
    stopped_generation: i64,
) -> anyhow::Result<Vec<ExecutionEnvironmentJob>> {
    let result = serde_json::to_value(super::types::superseded_result())?;
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/lifecycle/supersede_lifecycle_ensure_jobs.sql",
        application_id,
        environment_id,
        manager_id,
        stopped_generation,
        result,
    )
    .fetch_all(&mut **tx)
    .await?)
}
