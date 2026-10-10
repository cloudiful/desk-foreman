//! Lock-held and sealed-snapshot queries (issue 753 P2a).
//!
//! Every helper runs on an already-open transaction. Static SQL lives under
//! `src/sql/execution_environments/tasks/seeds/`.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::types::{SeedFile, TaskSeedRow};

/// The sealed snapshot header for one task, if it exists.
pub(super) async fn find_seed_header(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<TaskSeedRow>> {
    Ok(sqlx::query_file_as!(
        TaskSeedRow,
        "src/sql/execution_environments/tasks/seeds/find_seed_header.sql",
        task_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Insert one snapshot header. `None` means a racing seal already won.
pub(super) async fn insert_seed_header(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
    manifest_sha256: &str,
    file_count: u32,
    total_bytes: u32,
) -> anyhow::Result<Option<TaskSeedRow>> {
    Ok(sqlx::query_file_as!(
        TaskSeedRow,
        "src/sql/execution_environments/tasks/seeds/insert_seed_header.sql",
        task_id,
        manifest_sha256,
        i64::from(file_count),
        i64::from(total_bytes),
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Insert one sealed file. Files must be supplied in path order.
pub(super) async fn insert_seed_file(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
    path: &str,
    content: &str,
) -> anyhow::Result<()> {
    sqlx::query_file!(
        "src/sql/execution_environments/tasks/seeds/insert_seed_file.sql",
        task_id,
        path,
        content,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The sealed files of one snapshot, in canonical path order.
pub(super) async fn find_seed_files(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Vec<SeedFile>> {
    let rows = sqlx::query_file!(
        "src/sql/execution_environments/tasks/seeds/find_seed_files.sql",
        task_id,
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SeedFile {
            path: row.path,
            content: row.content,
        })
        .collect())
}
