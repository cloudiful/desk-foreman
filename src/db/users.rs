use anyhow::Context;
use sqlx::{AssertSqlSafe, PgPool, Row};

use crate::db::types::{
    CreateUserRequest, ListUsersParams, UpdateUserRequest, UserRecord, UserResponse,
};

pub async fn count_admin_users(pool: &PgPool) -> anyhow::Result<i64> {
    let row = sqlx::query(include_str!("../sql/count_admin_users.sql"))
        .fetch_one(pool)
        .await?;
    Ok(row.get("count"))
}

pub async fn bootstrap_admin(
    pool: &PgPool,
    login_name: &str,
    password_hash: &str,
    display_name: &str,
    email: &str,
    timezone: &str,
) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/bootstrap_admin.sql"))
        .bind(login_name)
        .bind(password_hash)
        .bind(display_name)
        .bind(email)
        .bind(timezone)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn find_user_by_login(
    pool: &PgPool,
    login_name: &str,
) -> anyhow::Result<Option<UserRecord>> {
    sqlx::query_as::<_, UserRecord>(include_str!("../sql/find_user_by_login.sql"))
        .bind(login_name)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn find_user_by_id(pool: &PgPool, user_id: i64) -> anyhow::Result<Option<UserRecord>> {
    sqlx::query_as::<_, UserRecord>(include_str!("../sql/find_user_by_id.sql"))
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn list_users(
    pool: &PgPool,
    params: &ListUsersParams,
) -> anyhow::Result<(Vec<UserResponse>, i64)> {
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);
    let sort_by = params.sort_by.as_deref().unwrap_or("updated_at");
    let sort_dir = params.sort_dir.as_deref().unwrap_or("desc");
    let list_sql = format!(
        "{} ORDER BY {sort_by} {sort_dir}, user_id DESC LIMIT $1 OFFSET $2",
        include_str!("../sql/list_users_base.sql")
    );
    let count_row = sqlx::query(include_str!("../sql/count_users.sql"))
        .fetch_one(pool)
        .await?;
    let total = count_row.get("count");
    let rows = sqlx::query_as::<_, UserResponse>(AssertSqlSafe(list_sql))
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .context("failed to list users")?;
    Ok((rows, total))
}

pub async fn create_user(
    pool: &PgPool,
    request: &CreateUserRequest,
    password_hash: &str,
) -> anyhow::Result<UserResponse> {
    sqlx::query_as::<_, UserResponse>(include_str!("../sql/create_user.sql"))
        .bind(&request.login_name)
        .bind(password_hash)
        .bind(&request.display_name)
        .bind(&request.email)
        .bind(&request.timezone)
        .bind(&request.workspace_root)
        .bind(request.is_admin)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn update_user_workspace(
    pool: &PgPool,
    user_id: i64,
    workspace_root: &str,
) -> anyhow::Result<Option<UserResponse>> {
    sqlx::query_as::<_, UserResponse>(include_str!("../sql/update_user_workspace.sql"))
        .bind(user_id)
        .bind(workspace_root)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn update_user(
    pool: &PgPool,
    user_id: i64,
    request: &UpdateUserRequest,
) -> anyhow::Result<Option<UserResponse>> {
    sqlx::query_as::<_, UserResponse>(include_str!("../sql/update_user.sql"))
        .bind(user_id)
        .bind(&request.display_name)
        .bind(&request.email)
        .bind(&request.timezone)
        .bind(request.is_admin)
        .bind(request.is_active)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn reset_user_password(
    pool: &PgPool,
    user_id: i64,
    password_hash: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(include_str!("../sql/reset_user_password.sql"))
        .bind(user_id)
        .bind(password_hash)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn change_password(
    pool: &PgPool,
    user_id: i64,
    password_hash: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(include_str!("../sql/change_password.sql"))
        .bind(user_id)
        .bind(password_hash)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn deactivate_user(pool: &PgPool, user_id: i64) -> anyhow::Result<bool> {
    let result = sqlx::query(include_str!("../sql/deactivate_user.sql"))
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
