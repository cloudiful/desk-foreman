//! Immutable authorized task input snapshot (issue 753 P2a).
//!
//! An authenticated application seals the bounded initial files it authorized
//! for an admitted task, and can read that snapshot back for later manager
//! consumption. The snapshot is immutable: there is no overwrite or delete
//! path, and the content digest deliberately excludes the task id and the
//! authorization generation so the same content hashes identically anywhere.
//!
//! This phase stores bytes in the control plane only. It dispatches no job,
//! marks nothing running, materializes no checkout and mounts nothing.

mod bounds;
mod queries;
mod read;
mod seal;
mod types;

use sqlx::PgPool;
use uuid::Uuid;

pub use bounds::plan_task_seed;
pub use types::{
    MAX_SEED_CONTENT_BYTES, MAX_SEED_ENTRIES, MAX_SEED_PATH_BYTES, MAX_SEED_TOTAL_BYTES,
    SeedBoundsError, SeedContent, SeedDescriptor, SeedError, SeedFile, SeedPlan,
};

/// Seal (or idempotently replay) one immutable input snapshot.
///
/// `plan` must come from [`plan_task_seed`], so the limits and path rules are
/// already proven when the transaction opens. A state refusal leaves nothing
/// written; the whole seal is one transaction.
pub async fn seal_native_task_seed(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    expected_generation: i64,
    plan: &SeedPlan,
) -> Result<SeedDescriptor, SeedError> {
    seal::seal(
        pool,
        application_id,
        environment_id,
        task_id,
        expected_generation,
        plan,
    )
    .await
}

/// The sealed snapshot's identity for `task_id`, if it exists.
///
/// Lock-held helper for preparation completion: the manager's ack must match
/// exactly this digest, count and byte total.
pub(crate) async fn find_native_task_seed_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: uuid::Uuid,
) -> anyhow::Result<Option<(String, u32, u32)>> {
    Ok(queries::find_seed_header(tx, task_id).await?.map(|row| {
        let descriptor = row.into_descriptor();
        (
            descriptor.manifest_sha256,
            descriptor.file_count,
            descriptor.total_bytes,
        )
    }))
}

/// True when a sealed snapshot header exists for `task_id`.
///
/// Lock-held helper for the preparation scheduler: it runs on the caller's
/// already-locked transaction, so a preparation is never enqueued without an
/// input to materialize.
pub(crate) async fn find_native_task_seed_header(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: uuid::Uuid,
) -> anyhow::Result<bool> {
    Ok(queries::find_seed_header(tx, task_id).await?.is_some())
}

/// Read one owned task's sealed snapshot, with its contents.
pub async fn find_native_task_seed(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<SeedContent, SeedError> {
    read::read(pool, application_id, environment_id, task_id).await
}

/// The seed identity a preparation advertises to the manager.
///
/// Owner-scoped: the environment and task are locked first, so a foreign or
/// absent resource is simply `None`.
#[derive(Clone, Debug)]
pub struct PreparationSeed {
    pub workspace_binding_id: i64,
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
}

/// Resolve the preparation seed identity for one owned task, if it exists.
pub async fn preparation_seed_identity(
    pool: &sqlx::PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<Option<PreparationSeed>, SeedError> {
    use crate::db::execution_environments::tasks::queries::{lock_environment, lock_task};

    let mut tx = pool.begin().await.map_err(types::internal)?;
    lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(types::internal)?
        .ok_or(SeedError::NotFound)?;
    let task = lock_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(types::internal)?
        .ok_or(SeedError::NotFound)?;
    let header = queries::find_seed_header(&mut tx, task_id)
        .await
        .map_err(types::internal)?;
    tx.commit().await.map_err(types::internal)?;
    Ok(header.map(|row| {
        let descriptor = row.into_descriptor();
        PreparationSeed {
            workspace_binding_id: task.workspace_binding_id,
            manifest_sha256: descriptor.manifest_sha256,
            file_count: descriptor.file_count,
            total_bytes: descriptor.total_bytes,
        }
    }))
}

/// The sealed seed of one preparation job, for its assigned manager.
///
/// Every fact is checked before content is read: the job is a live
/// `prepare_native_task` claim assigned to `manager_id`, and its task is still
/// `preparing` on the environment generation the job targeted. Anything else is
/// a not-found, so a foreign or expired job discloses nothing.
pub async fn preparation_seed_for_manager(
    pool: &sqlx::PgPool,
    manager_id: i64,
    job_id: Uuid,
) -> Result<SeedContent, SeedError> {
    let mut tx = pool.begin().await.map_err(types::internal)?;
    let row = sqlx::query_file!(
        "src/sql/execution_environments/tasks/prepare/find_seed_for_preparation_job.sql",
        job_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(types::internal)?;
    let Some(row) = row else {
        return Err(SeedError::NotFound);
    };
    if row.job_kind != "prepare_native_task"
        || row.assigned_manager_id != manager_id
        || row.task_manager_id != manager_id
        || row.job_state != "claimed"
        || row.task_state != "preparing"
        || row.active_native_task_id != Some(row.task_id)
        || row.environment_generation != row.job_generation
        || row.task_generation != row.job_generation
    {
        return Err(SeedError::NotFound);
    }
    let header = queries::find_seed_header(&mut tx, row.task_id)
        .await
        .map_err(types::internal)?
        .ok_or(SeedError::NotFound)?
        .into_descriptor();
    let files = queries::find_seed_files(&mut tx, row.task_id)
        .await
        .map_err(types::internal)?;
    tx.commit().await.map_err(types::internal)?;
    Ok(SeedContent {
        task_id: header.task_id,
        manifest_sha256: header.manifest_sha256,
        file_count: header.file_count,
        total_bytes: header.total_bytes,
        files,
    })
}
