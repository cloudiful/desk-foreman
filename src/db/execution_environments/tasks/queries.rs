//! Lock-held query helpers for durable native task persistence (issue 753 P1).
//!
//! Split from `tasks.rs` so the orchestration stays readable. Every helper
//! runs on an already-open transaction: admission locks the environment then
//! the binding, and holds both until it commits, so no legacy writer can
//! observe a half-admitted task.
//!
//! Static SQL lives under `src/sql/execution_environments/tasks/`.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::types::{NativeTask, TaskBinding, TaskEnvironment};

/// Lock and read the owner-scoped environment for one task mutation.
pub(super) async fn lock_environment(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<TaskEnvironment>> {
    Ok(sqlx::query_file_as!(
        TaskEnvironment,
        "src/sql/execution_environments/tasks/lock_task_environment.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock and read the authorized workspace binding for one task admission.
///
/// Locked second, and held to commit: a racing legacy write waits here and
/// then re-reads the native-active state instead of trusting its snapshot.
pub(super) async fn lock_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<TaskBinding>> {
    Ok(sqlx::query_file_as!(
        TaskBinding,
        "src/sql/execution_environments/tasks/lock_task_binding.sql",
        workspace_binding_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Any active task already holding this binding.
///
/// Read after the binding row lock is granted, so a task admitted while we
/// waited is visible. The partial unique index stays the backstop; this read is
/// what turns a shared-workspace conflict into a typed refusal instead of a
/// constraint violation.
pub(super) async fn find_active_task_for_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<Uuid>> {
    let row: Option<Uuid> = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/find_active_native_task_for_binding.sql",
        workspace_binding_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row)
}

/// One owned task by stable UUID.
pub(super) async fn find_task(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/find_task_by_id.sql",
        task_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Lock one owned task row for a cancel/release mutation.
pub(super) async fn lock_task(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/lock_task_by_id.sql",
        task_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// One owned task by opaque replay token.
///
/// Application-scoped: a foreign token is simply absent, so a caller can
/// never learn that another application's task exists.
pub(super) async fn find_task_by_token(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    operation_token: &str,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/find_task_by_token.sql",
        operation_token,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Insert one durable task intent. `None` means the id already exists and the
/// caller must re-read the winner.
pub(super) async fn insert_task(
    tx: &mut Transaction<'_, Postgres>,
    task: NewNativeTask<'_>,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/insert_task.sql",
        task.task_id,
        task.operation_token,
        task.environment_id,
        task.application_id,
        task.workspace_binding_id,
        task.subject_external_user_id,
        task.target_generation,
        task.assigned_manager_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// One new task intent to persist.
pub(super) struct NewNativeTask<'a> {
    pub(super) task_id: Uuid,
    pub(super) operation_token: &'a str,
    pub(super) environment_id: Uuid,
    pub(super) application_id: i64,
    pub(super) workspace_binding_id: i64,
    pub(super) subject_external_user_id: &'a str,
    pub(super) target_generation: i64,
    pub(super) assigned_manager_id: i64,
}

/// Point the environment at this task. `false` means another task already owns
/// it, which the caller reports as a conflict rather than overwriting.
pub(super) async fn set_environment_active_task(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<bool> {
    let affected = sqlx::query(include_str!(
        "../../../sql/execution_environments/tasks/set_environment_active_task.sql"
    ))
    .bind(task_id)
    .bind(environment_id)
    .bind(application_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok(affected == 1)
}

/// Clear the environment's active task pointer, only if it names this task.
pub(super) async fn clear_environment_active_task(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(include_str!(
        "../../../sql/execution_environments/tasks/clear_environment_active_task.sql"
    ))
    .bind(environment_id)
    .bind(application_id)
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Terminal release of a task whose stop proof was already verified.
pub(super) async fn mark_released(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/mark_task_released.sql",
        task_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Void a never-started task: released while provably nothing was dispatched.
pub(super) async fn mark_voided(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/mark_task_voided.sql",
        task_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Mark a task uncertain when a cancel cannot prove nothing was dispatched.
pub(super) async fn mark_uncertain(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/mark_task_uncertain.sql",
        task_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// How many durable jobs reference this task.
pub(super) async fn count_job_references(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> anyhow::Result<i64> {
    let row = sqlx::query_file!(
        "src/sql/execution_environments/tasks/count_task_job_references.sql",
        task_id,
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.references.unwrap_or(0))
}
