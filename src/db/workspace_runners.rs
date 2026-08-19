use chrono::{DateTime, Utc};
use runner_protocol::RunnerOwner;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::db::types::{ListWorkspaceRunnersParams, Page};

#[derive(Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct WorkspaceRunnerRecord {
    pub runner_id: i64,
    pub owner_kind: String,
    pub owner_user_id: Option<i64>,
    pub owner_workspace_binding_id: Option<i64>,
    pub container_name: String,
    pub container_id: Option<String>,
    pub runtime: String,
    pub runtime_class: Option<String>,
    pub image_name: String,
    pub status: String,
    pub network_enabled: bool,
    pub workspace_root: String,
    pub last_active_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SaveWorkspaceRunner<'a> {
    pub owner: &'a RunnerOwner,
    pub container_name: &'a str,
    pub container_id: Option<&'a str>,
    pub runtime: &'a str,
    pub runtime_class: Option<&'a str>,
    pub image_name: &'a str,
    pub status: &'a str,
    pub network_enabled: bool,
    pub workspace_root: &'a str,
    pub last_error: Option<&'a str>,
}

pub async fn find_workspace_runner_by_owner(
    pool: &PgPool,
    owner: &RunnerOwner,
) -> anyhow::Result<Option<WorkspaceRunnerRecord>> {
    match owner {
        RunnerOwner::InternalUser { user_id } => sqlx::query_as::<_, WorkspaceRunnerRecord>(
            r#"
            SELECT *
            FROM workspace_runners
            WHERE owner_kind = 'user' AND owner_user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into),
        RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        } => sqlx::query_as::<_, WorkspaceRunnerRecord>(
            r#"
            SELECT *
            FROM workspace_runners
            WHERE owner_kind = 'workspace_binding' AND owner_workspace_binding_id = $1
            "#,
        )
        .bind(workspace_binding_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into),
    }
}

pub async fn list_workspace_runners(
    pool: &PgPool,
    params: &ListWorkspaceRunnersParams,
) -> anyhow::Result<Page<WorkspaceRunnerRecord>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_workspace_runners.sql"))
        .bind(&params.status)
        .bind(&params.owner_kind)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, WorkspaceRunnerRecord>(include_str!(
        "../sql/list_workspace_runners.sql"
    ))
    .bind(&params.status)
    .bind(&params.owner_kind)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn operations_summary(
    pool: &PgPool,
) -> anyhow::Result<(i64, i64, i64, i64, i64, i64, i64)> {
    #[derive(sqlx::FromRow)]
    struct Row {
        active_runners: i64,
        failed_operations: i64,
        archived_workspaces: i64,
        runner_managers_total: i64,
        runner_managers_online: i64,
        runner_managers_offline: i64,
        runner_managers_disabled: i64,
    }
    let row = sqlx::query_as::<_, Row>(include_str!("../sql/operations_summary.sql"))
        .bind(runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
        .fetch_one(pool)
        .await?;
    Ok((
        row.active_runners,
        row.failed_operations,
        row.archived_workspaces,
        row.runner_managers_total,
        row.runner_managers_online,
        row.runner_managers_offline,
        row.runner_managers_disabled,
    ))
}

pub async fn list_stale_workspace_runners(
    pool: &PgPool,
    idle_before: DateTime<Utc>,
) -> anyhow::Result<Vec<WorkspaceRunnerRecord>> {
    sqlx::query_as::<_, WorkspaceRunnerRecord>(
        r#"
        SELECT *
        FROM workspace_runners
        WHERE status = 'running' AND last_active_at < $1
        ORDER BY last_active_at ASC
        "#,
    )
    .bind(idle_before)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn save_workspace_runner(
    pool: &PgPool,
    input: SaveWorkspaceRunner<'_>,
) -> anyhow::Result<WorkspaceRunnerRecord> {
    let (owner_kind, owner_user_id, owner_workspace_binding_id) = match input.owner {
        RunnerOwner::InternalUser { user_id } => ("user", Some(*user_id), None),
        RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        } => ("workspace_binding", None, Some(*workspace_binding_id)),
    };

    sqlx::query_as::<_, WorkspaceRunnerRecord>(
        r#"
        INSERT INTO workspace_runners (
            owner_kind,
            owner_user_id,
            owner_workspace_binding_id,
            container_name,
            container_id,
            runtime,
            runtime_class,
            image_name,
            status,
            network_enabled,
            workspace_root,
            last_active_at,
            updated_at,
            last_error
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), NOW(), $12
        )
        ON CONFLICT (container_name) DO UPDATE
        SET
            owner_kind = EXCLUDED.owner_kind,
            owner_user_id = EXCLUDED.owner_user_id,
            owner_workspace_binding_id = EXCLUDED.owner_workspace_binding_id,
            container_id = EXCLUDED.container_id,
            runtime = EXCLUDED.runtime,
            runtime_class = EXCLUDED.runtime_class,
            image_name = EXCLUDED.image_name,
            status = EXCLUDED.status,
            network_enabled = EXCLUDED.network_enabled,
            workspace_root = EXCLUDED.workspace_root,
            last_active_at = NOW(),
            updated_at = NOW(),
            last_error = EXCLUDED.last_error
        RETURNING *
        "#,
    )
    .bind(owner_kind)
    .bind(owner_user_id)
    .bind(owner_workspace_binding_id)
    .bind(input.container_name)
    .bind(input.container_id)
    .bind(input.runtime)
    .bind(input.runtime_class)
    .bind(input.image_name)
    .bind(input.status)
    .bind(input.network_enabled)
    .bind(input.workspace_root)
    .bind(input.last_error)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn touch_workspace_runner(pool: &PgPool, owner: &RunnerOwner) -> anyhow::Result<()> {
    match owner {
        RunnerOwner::InternalUser { user_id } => {
            sqlx::query(
                r#"
                UPDATE workspace_runners
                SET last_active_at = NOW(), updated_at = NOW()
                WHERE owner_kind = 'user' AND owner_user_id = $1
                "#,
            )
            .bind(user_id)
            .execute(pool)
            .await?;
        }
        RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        } => {
            sqlx::query(
                r#"
                UPDATE workspace_runners
                SET last_active_at = NOW(), updated_at = NOW()
                WHERE owner_kind = 'workspace_binding' AND owner_workspace_binding_id = $1
                "#,
            )
            .bind(workspace_binding_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}
