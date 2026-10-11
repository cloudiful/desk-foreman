//! Persistence for durable execution environments (issue 737 P1).
//!
//! `ensure` uses a unique-index-safe insert with `ON CONFLICT DO NOTHING`
//! so concurrent callers converge on one row. `invalidate` uses an
//! owner-scoped atomic conditional update so a stale generation never
//! mutates. Static SQL lives under `src/sql/execution_environments/` and
//! is checked with `query_file_as!`.

use sqlx::PgPool;
use uuid::Uuid;

use super::types::{ExecutionEnvironment, derive_state_volume_ref};

/// Outcome of an idempotent ensure.
#[derive(Debug)]
pub enum EnsureOutcome {
    /// A new row was inserted.
    Created(ExecutionEnvironment),
    /// The existing row was returned unchanged.
    Existing(ExecutionEnvironment),
    /// The existing row uses a different `runtime_kind`; no mutation ran.
    RuntimeConflict { current: ExecutionEnvironment },
}

/// Outcome of an owner-scoped invalidate.
#[derive(Debug)]
pub enum InvalidateOutcome {
    /// Generation bumped and state set to `stopping`.
    Invalidated(ExecutionEnvironment),
    /// Row exists but generation mismatched; no mutation ran.
    Stale { current: ExecutionEnvironment },
    /// No row for this application and environment id.
    NotFound,
    /// An active native task owns the environment.
    NativeTaskActive,
}

/// Idempotent ensure for one `(application_id, external_user_id)` pair.
///
/// Trims the opaque inputs, inserts a fresh UUID identity with a derived
/// `df-env-<uuid-simple>` reference, and on conflict returns the existing
/// row unchanged. A `runtime_kind` mismatch returns `RuntimeConflict`
/// without mutating.
pub async fn ensure_execution_environment(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    runtime_kind: &str,
) -> anyhow::Result<EnsureOutcome> {
    let external_user_id = external_user_id.trim().to_string();
    let runtime_kind = runtime_kind.trim().to_string();
    let environment_id = Uuid::new_v4();
    let state_volume_ref = derive_state_volume_ref(&environment_id);

    let inserted: Option<ExecutionEnvironment> = sqlx::query_file_as!(
        ExecutionEnvironment,
        "src/sql/execution_environments/insert_execution_environment.sql",
        environment_id,
        application_id,
        external_user_id,
        runtime_kind,
        state_volume_ref,
    )
    .fetch_optional(pool)
    .await?;

    if let Some(row) = inserted {
        return Ok(EnsureOutcome::Created(row));
    }

    let current: Option<ExecutionEnvironment> = sqlx::query_file_as!(
        ExecutionEnvironment,
        "src/sql/execution_environments/find_execution_environment_by_identity.sql",
        application_id,
        external_user_id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(current) = current else {
        anyhow::bail!("execution environment vanished after upsert conflict");
    };

    if current.runtime_kind != runtime_kind {
        return Ok(EnsureOutcome::RuntimeConflict { current });
    }

    Ok(EnsureOutcome::Existing(current))
}

/// Owner-scoped lookup by durable identity.
pub async fn find_execution_environment(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<ExecutionEnvironment>> {
    sqlx::query_file_as!(
        ExecutionEnvironment,
        "src/sql/execution_environments/find_execution_environment_by_id.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Owner-scoped lookup by `(application_id, external_user_id)`.
pub async fn find_execution_environment_by_identity(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
) -> anyhow::Result<Option<ExecutionEnvironment>> {
    sqlx::query_file_as!(
        ExecutionEnvironment,
        "src/sql/execution_environments/find_execution_environment_by_identity.sql",
        application_id,
        external_user_id.trim(),
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Atomic owner-scoped invalidate.
///
/// Bumps `generation` and sets `state = 'stopping'` only when the stored
/// generation equals `expected_generation`. A mismatch leaves the row
/// unchanged and returns `Stale`; an unknown or foreign id returns
/// `NotFound` without mutation.
pub async fn invalidate_execution_environment(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
) -> anyhow::Result<InvalidateOutcome> {
    let mut tx = pool.begin().await?;
    let active_task = sqlx::query_file_scalar!(
        "src/sql/execution_environments/lock_environment_for_invalidation.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(active_task) = active_task else {
        tx.commit().await?;
        return Ok(InvalidateOutcome::NotFound);
    };
    if active_task {
        tx.commit().await?;
        return Ok(InvalidateOutcome::NativeTaskActive);
    }

    let updated: Option<ExecutionEnvironment> = sqlx::query_file_as!(
        ExecutionEnvironment,
        "src/sql/execution_environments/invalidate_execution_environment.sql",
        environment_id,
        application_id,
        expected_generation,
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    if let Some(row) = updated {
        return Ok(InvalidateOutcome::Invalidated(row));
    }

    let current = find_execution_environment(pool, application_id, environment_id).await?;
    match current {
        None => Ok(InvalidateOutcome::NotFound),
        Some(current) => Ok(InvalidateOutcome::Stale { current }),
    }
}
