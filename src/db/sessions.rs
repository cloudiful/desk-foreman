use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    sea_query::{Expr, ExprTrait},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::native::{application_token, mcp_token, web_session};
use crate::db::types::{ApplicationTokenRecord, McpTokenPolicy, UserRecord, WebSessionRecord};

fn native(pool: &PgPool) -> DatabaseConnection {
    DatabaseConnection::from(pool.clone())
}

fn session_record(model: web_session::Model) -> WebSessionRecord {
    WebSessionRecord {
        session_id: model.session_id,
        user_id: model.user_id,
        expires_at: model.expires_at,
        created_at: model.created_at,
        last_seen_at: model.last_seen_at,
        revoked_at: model.revoked_at,
    }
}

pub async fn create_session(
    pool: &PgPool,
    session_id: Uuid,
    user_id: i64,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    web_session::Entity::insert(web_session::ActiveModel {
        session_id: sea_orm::Set(session_id),
        user_id: sea_orm::Set(user_id),
        expires_at: sea_orm::Set(expires_at),
        ..Default::default()
    })
    .exec_without_returning(&native(pool))
    .await?;
    Ok(())
}

pub async fn revoke_session(pool: &PgPool, session_id: Uuid) -> anyhow::Result<()> {
    web_session::Entity::update_many()
        .col_expr(web_session::Column::RevokedAt, Expr::cust("NOW()"))
        .filter(web_session::Column::SessionId.eq(session_id))
        .filter(web_session::Column::RevokedAt.is_null())
        .exec(&native(pool))
        .await?;
    Ok(())
}

pub async fn find_active_session(
    pool: &PgPool,
    session_id: Uuid,
) -> anyhow::Result<Option<(WebSessionRecord, UserRecord)>> {
    Ok(web_session::Entity::find()
        .find_both_related(crate::db::native::user::Entity)
        .filter(web_session::Column::SessionId.eq(session_id))
        .filter(web_session::Column::RevokedAt.is_null())
        .filter(Expr::col(web_session::Column::ExpiresAt).gt(Expr::cust("NOW()")))
        .one(&native(pool))
        .await?
        .map(|(session, user)| (session_record(session), crate::db::users::record(user))))
}

pub async fn touch_session(pool: &PgPool, session_id: Uuid) -> anyhow::Result<()> {
    web_session::Entity::update_many()
        .col_expr(web_session::Column::LastSeenAt, Expr::cust("NOW()"))
        .filter(web_session::Column::SessionId.eq(session_id))
        .filter(web_session::Column::RevokedAt.is_null())
        .exec(&native(pool))
        .await?;
    Ok(())
}

pub async fn update_last_login(pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    crate::db::native::user::Entity::update_many()
        .col_expr(
            crate::db::native::user::Column::LastLoginAt,
            Expr::cust("NOW()"),
        )
        .col_expr(
            crate::db::native::user::Column::UpdatedAt,
            Expr::cust("NOW()"),
        )
        .filter(crate::db::native::user::Column::UserId.eq(user_id))
        .exec(&native(pool))
        .await?;
    Ok(())
}

pub async fn revoke_user_sessions(pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    web_session::Entity::update_many()
        .col_expr(web_session::Column::RevokedAt, Expr::cust("NOW()"))
        .filter(web_session::Column::UserId.eq(user_id))
        .filter(web_session::Column::RevokedAt.is_null())
        .exec(&native(pool))
        .await?;
    Ok(())
}

pub async fn find_active_mcp_token_hash(
    pool: &PgPool,
    token_hash: &str,
) -> anyhow::Result<Option<(UserRecord, McpTokenPolicy)>> {
    Ok(mcp_token::Entity::find()
        .find_both_related(crate::db::native::user::Entity)
        .filter(mcp_token::Column::TokenHash.eq(token_hash))
        .filter(mcp_token::Column::IsActive.eq(true))
        .filter(
            mcp_token::Column::ExpiresAt
                .is_null()
                .or(Expr::col(mcp_token::Column::ExpiresAt).gt(Expr::cust("NOW()"))),
        )
        .filter(crate::db::native::user::Column::DeletedAt.is_null())
        .filter(crate::db::native::user::Column::IsActive.eq(true))
        .one(&native(pool))
        .await?
        .map(|(token, user)| {
            (
                crate::db::users::record(user),
                McpTokenPolicy {
                    expires_at: token.expires_at,
                    scopes: token.scopes,
                    max_timeout_ms: token.max_timeout_ms,
                    max_output_bytes: token.max_output_bytes,
                    max_file_bytes: token.max_file_bytes,
                    max_sessions: token.max_sessions,
                    network_enabled: token.network_enabled,
                },
            )
        }))
}

pub async fn touch_mcp_token(pool: &PgPool, token_hash: &str) -> anyhow::Result<()> {
    mcp_token::Entity::update_many()
        .col_expr(mcp_token::Column::LastUsedAt, Expr::cust("NOW()"))
        .filter(mcp_token::Column::TokenHash.eq(token_hash))
        .filter(mcp_token::Column::IsActive.eq(true))
        .exec(&native(pool))
        .await?;
    Ok(())
}

pub async fn find_active_application_token_hash(
    pool: &PgPool,
    token_hash: &str,
) -> anyhow::Result<Option<ApplicationTokenRecord>> {
    Ok(application_token::Entity::find()
        .find_both_related(crate::db::native::application::Entity)
        .filter(application_token::Column::TokenHash.eq(token_hash))
        .filter(application_token::Column::IsActive.eq(true))
        .filter(
            application_token::Column::ExpiresAt
                .is_null()
                .or(Expr::col(application_token::Column::ExpiresAt).gt(Expr::cust("NOW()"))),
        )
        .filter(crate::db::native::application::Column::IsActive.eq(true))
        .one(&native(pool))
        .await?
        .map(|(token, application)| ApplicationTokenRecord {
            token_id: token.token_id,
            application_id: application.application_id,
            name: application.name,
            is_active: application.is_active,
            workspace_template: application.workspace_template,
            default_shell: application.default_shell,
            created_at: application.created_at,
            updated_at: application.updated_at,
            default_scopes: application.default_scopes,
            app_max_timeout_ms: application.max_timeout_ms,
            app_max_output_bytes: application.max_output_bytes,
            app_max_file_bytes: application.max_file_bytes,
            app_max_sessions: application.max_sessions,
            app_network_enabled: application.network_enabled,
            expires_at: token.expires_at,
            scopes: token.scopes,
            max_timeout_ms: token.max_timeout_ms,
            max_output_bytes: token.max_output_bytes,
            max_file_bytes: token.max_file_bytes,
            max_sessions: token.max_sessions,
            network_enabled: token.network_enabled,
        }))
}

pub async fn touch_application_token(pool: &PgPool, token_hash: &str) -> anyhow::Result<()> {
    application_token::Entity::update_many()
        .col_expr(application_token::Column::LastUsedAt, Expr::cust("NOW()"))
        .filter(application_token::Column::TokenHash.eq(token_hash))
        .exec(&native(pool))
        .await?;
    Ok(())
}
