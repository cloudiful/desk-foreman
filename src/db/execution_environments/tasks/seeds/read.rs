//! Reading one sealed snapshot (issue 753 P2a).
//!
//! The read is application-plus-environment-plus-task scoped before any
//! content is fetched, so a foreign or absent task is a plain not-found and
//! the caller never learns whether another tenant's snapshot exists.
//!
//! The snapshot is artifact history, so it is returned unchanged for a task in
//! any state — including `released` and `uncertain` — because the bytes were
//! authorized once and must stay readable for later reconciliation.

use sqlx::PgPool;
use uuid::Uuid;

use super::queries::{find_seed_files, find_seed_header};
use super::types::internal;
use super::types::{SeedContent, SeedError};
use crate::db::execution_environments::tasks::queries::{
    find_task, lock_binding, lock_environment,
};

/// Read the sealed snapshot of one owned task.
pub(super) async fn read(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<SeedContent, SeedError> {
    let mut tx = pool.begin().await.map_err(internal)?;

    // Same lock order as every other task mutation: the environment first.
    lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?
        .ok_or(SeedError::NotFound)?;

    let task = find_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?
        .ok_or(SeedError::NotFound)?;
    // The task's binding must still belong to this application: the snapshot is
    // never readable across an ownership boundary.
    lock_binding(&mut tx, task.workspace_binding_id)
        .await
        .map_err(internal)?
        .filter(|binding| binding.application_id == application_id)
        .ok_or(SeedError::NotFound)?;

    let header = find_seed_header(&mut tx, task_id)
        .await
        .map_err(internal)?
        .ok_or(SeedError::NotFound)?
        .into_descriptor();
    let files = find_seed_files(&mut tx, task_id).await.map_err(internal)?;

    tx.commit().await.map_err(internal)?;
    Ok(SeedContent {
        task_id: header.task_id,
        manifest_sha256: header.manifest_sha256,
        file_count: header.file_count,
        total_bytes: header.total_bytes,
        files,
    })
}
