use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::db::types::{ApplicationTokenRecord, McpTokenPolicy, UserRecord, WebSessionRecord};

pub async fn create_session(
    pool: &PgPool,
    session_id: Uuid,
    user_id: i64,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/create_web_session.sql"))
        .bind(session_id)
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_session(pool: &PgPool, session_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/revoke_web_session.sql"))
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn find_active_session(
    pool: &PgPool,
    session_id: Uuid,
) -> anyhow::Result<Option<(WebSessionRecord, UserRecord)>> {
    let row = sqlx::query(include_str!("../sql/find_active_session.sql"))
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| {
        (
            WebSessionRecord {
                session_id: row.get("session_id"),
                user_id: row.get("user_id"),
                expires_at: row.get("expires_at"),
                created_at: row.get("created_at"),
                last_seen_at: row.get("last_seen_at"),
                revoked_at: row.get("revoked_at"),
            },
            UserRecord {
                user_id: row.get("user_id"),
                login_name: row.get("login_name"),
                password_hash: row.get("password_hash"),
                display_name: row.get("display_name"),
                email: row.get("email"),
                timezone: row.get("timezone"),
                workspace_root: row.get("workspace_root"),
                is_admin: row.get("is_admin"),
                is_active: row.get("is_active"),
                must_change_password: row.get("must_change_password"),
                deleted_at: row.get("deleted_at"),
                last_login_at: row.get("last_login_at"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            },
        )
    }))
}

pub async fn touch_session(pool: &PgPool, session_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_web_session.sql"))
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_last_login(pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/update_last_login.sql"))
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_user_sessions(pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/revoke_user_sessions.sql"))
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn find_active_mcp_token_hash(
    pool: &PgPool,
    token_hash: &str,
) -> anyhow::Result<Option<(UserRecord, McpTokenPolicy)>> {
    let row = sqlx::query(include_str!("../sql/find_active_mcp_token.sql"))
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| {
        (
            UserRecord {
                user_id: row.get("user_id"),
                login_name: row.get("login_name"),
                password_hash: row.get("password_hash"),
                display_name: row.get("display_name"),
                email: row.get("email"),
                timezone: row.get("timezone"),
                workspace_root: row.get("workspace_root"),
                is_admin: row.get("is_admin"),
                is_active: row.get("is_active"),
                must_change_password: row.get("must_change_password"),
                deleted_at: row.get("deleted_at"),
                last_login_at: row.get("last_login_at"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            },
            McpTokenPolicy {
                expires_at: row.get("expires_at"),
                scopes: row.get("scopes"),
                max_timeout_ms: row.get("max_timeout_ms"),
                max_output_bytes: row.get("max_output_bytes"),
                max_file_bytes: row.get("max_file_bytes"),
                max_sessions: row.get("max_sessions"),
                network_enabled: row.get("network_enabled"),
            },
        )
    }))
}

pub async fn touch_mcp_token(pool: &PgPool, token_hash: &str) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_mcp_token.sql"))
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn find_active_application_token_hash(
    pool: &PgPool,
    token_hash: &str,
) -> anyhow::Result<Option<ApplicationTokenRecord>> {
    sqlx::query_as::<_, ApplicationTokenRecord>(include_str!(
        "../sql/find_active_application_token.sql"
    ))
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn touch_application_token(pool: &PgPool, token_hash: &str) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_application_token.sql"))
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}
