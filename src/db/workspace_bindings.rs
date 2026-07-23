use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::db::types::{ListWorkspaceBindingsParams, WorkspaceBindingResponse};

pub async fn find_workspace_binding(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!("../sql/find_workspace_binding.sql"))
        .bind(application_id)
        .bind(external_user_id)
        .bind(workspace_key)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn find_workspace_binding_any(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_any.sql"
    ))
    .bind(application_id)
    .bind(external_user_id)
    .bind(workspace_key)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn create_workspace_binding(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
    workspace_root: &str,
) -> anyhow::Result<WorkspaceBindingResponse> {
    let external_user_hash = external_user_hash(external_user_id);
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/create_workspace_binding.sql"
    ))
    .bind(application_id)
    .bind(external_user_id)
    .bind(workspace_key)
    .bind(external_user_hash)
    .bind(workspace_root)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn touch_workspace_binding(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_workspace_binding.sql"))
        .bind(workspace_binding_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_workspace_binding_state(
    pool: &PgPool,
    workspace_binding_id: i64,
    state: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/set_workspace_binding_state.sql"
    ))
    .bind(workspace_binding_id)
    .bind(state)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_archived_workspace_bindings(
    pool: &PgPool,
    archived_before: DateTime<Utc>,
) -> anyhow::Result<Vec<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/list_archived_workspace_bindings.sql"
    ))
    .bind(archived_before)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn delete_workspace_binding(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<String>> {
    sqlx::query(include_str!("../sql/delete_workspace_binding.sql"))
        .bind(workspace_binding_id)
        .fetch_optional(pool)
        .await
        .map(|row| row.map(|row| row.get("workspace_root")))
        .map_err(Into::into)
}

pub async fn find_workspace_binding_by_id(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_by_id.sql"
    ))
    .bind(workspace_binding_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_workspace_bindings(
    pool: &PgPool,
    params: &ListWorkspaceBindingsParams,
) -> anyhow::Result<(Vec<WorkspaceBindingResponse>, i64)> {
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);
    let total_row = sqlx::query(include_str!("../sql/count_workspace_bindings.sql"))
        .bind(params.application_id)
        .bind(&params.external_user_id)
        .bind(&params.workspace_key)
        .bind(params.is_active)
        .fetch_one(pool)
        .await?;
    let total = total_row.get("count");
    let rows = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/list_workspace_bindings.sql"
    ))
    .bind(params.application_id)
    .bind(&params.external_user_id)
    .bind(&params.workspace_key)
    .bind(params.is_active)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

pub fn external_user_hash(external_user_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(external_user_id.as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
