//! Atomic completion of durable native-task preparations (issue 753 P3r).

mod transaction;
mod writes;

use runner_protocol::NativeTaskPreparedResult;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::execution_environments::NativeTask;

/// One preparation completion request from the authenticated manager.
pub(crate) struct PreparationCompletion {
    pub manager_id: i64,
    pub job_id: Uuid,
    pub result: Option<NativeTaskPreparedResult>,
    pub failed: bool,
}

/// Terminal outcome of completing a preparation.
#[derive(Debug)]
pub(crate) enum PreparationCompletionOutcome {
    /// The task is now `prepared`.
    Prepared,
    /// The manager's failed preparation was atomically recorded as uncertain.
    Uncertain,
    /// The ack did not match the job/task/seed; nothing was mutated.
    Inconsistent(&'static str),
    /// No such durable job.
    NotFound,
    /// The authenticated manager is not the assigned manager.
    ForeignManager,
    /// The job or task was not in a completable state.
    NotCompletable,
}

/// Complete one successful preparation reported by the authenticated manager.
pub(crate) async fn complete_native_task_preparation(
    pool: &PgPool,
    completion: PreparationCompletion,
) -> anyhow::Result<PreparationCompletionOutcome> {
    transaction::complete(pool, completion).await
}

/// Mark the task linked to a preparation job `uncertain` (expiry/failure).
pub(crate) async fn mark_preparation_uncertain(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/prepare/mark_linked_task_uncertain.sql",
        task_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}
