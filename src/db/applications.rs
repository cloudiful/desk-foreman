use anyhow::Context;
use rand::{RngExt, distr::Alphanumeric};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Select, sea_query::Expr,
};
use sqlx::PgPool;

use crate::db::native::{application, application_token, mcp_token};
use crate::db::types::{
    ApplicationResponse, ApplicationTokenResponse, CreateApplicationRequest,
    CreateApplicationTokenRequest, CreateMcpTokenRequest, ListApplicationTokensParams,
    ListApplicationsParams, ListMcpTokensParams, McpTokenResponse, Page, UpdateApplicationRequest,
    UpdateApplicationTokenRequest, UpdateMcpTokenRequest,
};
use crate::policy::ALL_SCOPES;

fn native(pool: &PgPool) -> DatabaseConnection {
    DatabaseConnection::from(pool.clone())
}

fn mcp_token_response(model: mcp_token::Model) -> McpTokenResponse {
    McpTokenResponse {
        token_id: model.token_id,
        token_name: model.token_name,
        user_id: model.created_by,
        is_active: model.is_active,
        created_at: model.created_at,
        last_used_at: model.last_used_at,
        expires_at: model.expires_at,
        scopes: model.scopes,
        max_timeout_ms: model.max_timeout_ms,
        max_output_bytes: model.max_output_bytes,
        max_file_bytes: model.max_file_bytes,
        max_sessions: model.max_sessions,
        network_enabled: model.network_enabled,
    }
}

fn application_response(model: application::Model) -> ApplicationResponse {
    ApplicationResponse {
        application_id: model.application_id,
        name: model.name,
        is_active: model.is_active,
        workspace_template: model.workspace_template,
        default_shell: model.default_shell,
        created_at: model.created_at,
        updated_at: model.updated_at,
        default_scopes: model.default_scopes,
        max_timeout_ms: model.max_timeout_ms,
        max_output_bytes: model.max_output_bytes,
        max_file_bytes: model.max_file_bytes,
        max_sessions: model.max_sessions,
        network_enabled: model.network_enabled,
    }
}

fn application_token_response(model: application_token::Model) -> ApplicationTokenResponse {
    ApplicationTokenResponse {
        token_id: model.token_id,
        application_id: model.application_id,
        token_name: model.token_name,
        is_active: model.is_active,
        created_at: model.created_at,
        last_used_at: model.last_used_at,
        expires_at: model.expires_at,
        scopes: model.scopes,
        max_timeout_ms: model.max_timeout_ms,
        max_output_bytes: model.max_output_bytes,
        max_file_bytes: model.max_file_bytes,
        max_sessions: model.max_sessions,
        network_enabled: model.network_enabled,
    }
}

fn mcp_token_query(
    params: &ListMcpTokensParams,
    is_active: Option<bool>,
) -> Select<mcp_token::Entity> {
    let mut query = mcp_token::Entity::find();
    if let Some(search) = params.search.as_deref() {
        query = query.filter(mcp_token::Column::TokenName.ilike(format!("%{search}%")));
    }
    if let Some(user_id) = params.user_id {
        query = query.filter(mcp_token::Column::CreatedBy.eq(user_id));
    }
    if let Some(is_active) = is_active {
        query = query.filter(mcp_token::Column::IsActive.eq(is_active));
    }
    query
}

fn application_query(params: &ListApplicationsParams) -> Select<application::Entity> {
    let mut query = application::Entity::find();
    if let Some(search) = params.search.as_deref() {
        query = query.filter(application::Column::Name.ilike(format!("%{search}%")));
    }
    if let Some(is_active) = params.is_active {
        query = query.filter(application::Column::IsActive.eq(is_active));
    }
    query
}

fn application_token_query(
    params: &ListApplicationTokensParams,
    is_active: Option<bool>,
) -> Select<application_token::Entity> {
    let mut query = application_token::Entity::find();
    if let Some(application_id) = params.application_id {
        query = query.filter(application_token::Column::ApplicationId.eq(application_id));
    }
    if let Some(is_active) = is_active {
        query = query.filter(application_token::Column::IsActive.eq(is_active));
    }
    query
}

pub async fn list_mcp_tokens(
    pool: &PgPool,
    params: &ListMcpTokensParams,
) -> anyhow::Result<Page<McpTokenResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let is_active = params.is_active.or(Some(true));
    let db = native(pool);
    let total = mcp_token_query(params, is_active).count(&db).await? as i64;
    let items = mcp_token_query(params, is_active)
        .order_by(mcp_token::Column::CreatedAt, sea_orm::Order::Desc)
        .order_by(mcp_token::Column::TokenId, sea_orm::Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(&db)
        .await?
        .into_iter()
        .map(mcp_token_response)
        .collect();
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn find_mcp_token_by_id(
    pool: &PgPool,
    token_id: i64,
) -> anyhow::Result<Option<McpTokenResponse>> {
    Ok(mcp_token::Entity::find_by_id(token_id)
        .one(&native(pool))
        .await?
        .map(mcp_token_response))
}

pub async fn create_mcp_token(
    pool: &PgPool,
    request: &CreateMcpTokenRequest,
) -> anyhow::Result<(String, McpTokenResponse)> {
    let token = generate_token();
    let token_hash = crate::auth::hash_bearer_token(&token);
    let scopes = request.scopes.clone().unwrap_or_else(all_scopes);
    let model = mcp_token::Entity::insert(mcp_token::ActiveModel {
        token_name: sea_orm::Set(request.token_name.clone()),
        token_hash: sea_orm::Set(token_hash),
        created_by: sea_orm::Set(request.user_id),
        is_active: sea_orm::Set(true),
        expires_at: sea_orm::Set(request.expires_at),
        scopes: sea_orm::Set(scopes),
        max_timeout_ms: sea_orm::Set(request.max_timeout_ms),
        max_output_bytes: sea_orm::Set(request.max_output_bytes),
        max_file_bytes: sea_orm::Set(request.max_file_bytes),
        max_sessions: sea_orm::Set(request.max_sessions),
        network_enabled: sea_orm::Set(request.network_enabled.unwrap_or(true)),
        ..Default::default()
    })
    .exec_with_returning(&native(pool))
    .await?;
    Ok((token, mcp_token_response(model)))
}

pub async fn revoke_mcp_token(pool: &PgPool, token_id: i64) -> anyhow::Result<bool> {
    let result = mcp_token::Entity::update_many()
        .col_expr(mcp_token::Column::IsActive, Expr::val(false))
        .filter(mcp_token::Column::TokenId.eq(token_id))
        .filter(mcp_token::Column::IsActive.eq(true))
        .exec(&native(pool))
        .await?;
    Ok(result.rows_affected > 0)
}

fn assignment<C, T>(column: C, value: Option<T>) -> Expr
where
    C: ColumnTrait,
    T: Into<sea_orm::Value>,
{
    match value {
        Some(value) => Expr::val(value),
        None => column.into_expr(),
    }
}

fn mcp_token_update(
    request: &UpdateMcpTokenRequest,
) -> sea_orm::query::UpdateMany<mcp_token::Entity> {
    mcp_token::Entity::update_many()
        .col_expr(
            mcp_token::Column::ExpiresAt,
            assignment(mcp_token::Column::ExpiresAt, request.expires_at),
        )
        .col_expr(
            mcp_token::Column::Scopes,
            assignment(mcp_token::Column::Scopes, request.scopes.clone()),
        )
        .col_expr(
            mcp_token::Column::MaxTimeoutMs,
            assignment(mcp_token::Column::MaxTimeoutMs, request.max_timeout_ms),
        )
        .col_expr(
            mcp_token::Column::MaxOutputBytes,
            assignment(mcp_token::Column::MaxOutputBytes, request.max_output_bytes),
        )
        .col_expr(
            mcp_token::Column::MaxFileBytes,
            assignment(mcp_token::Column::MaxFileBytes, request.max_file_bytes),
        )
        .col_expr(
            mcp_token::Column::MaxSessions,
            assignment(mcp_token::Column::MaxSessions, request.max_sessions),
        )
        .col_expr(
            mcp_token::Column::NetworkEnabled,
            assignment(mcp_token::Column::NetworkEnabled, request.network_enabled),
        )
}

pub async fn update_mcp_token(
    pool: &PgPool,
    token_id: i64,
    request: &UpdateMcpTokenRequest,
) -> anyhow::Result<Option<McpTokenResponse>> {
    let model = mcp_token_update(request)
        .filter(mcp_token::Column::TokenId.eq(token_id))
        .exec_with_returning(&native(pool))
        .await?
        .into_iter()
        .next();
    Ok(model.map(mcp_token_response))
}

pub async fn list_applications(
    pool: &PgPool,
    params: &ListApplicationsParams,
) -> anyhow::Result<Page<ApplicationResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let db = native(pool);
    let total = application_query(params).count(&db).await? as i64;
    let items = application_query(params)
        .order_by(application::Column::CreatedAt, sea_orm::Order::Desc)
        .order_by(application::Column::ApplicationId, sea_orm::Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(&db)
        .await
        .context("failed to list applications")?
        .into_iter()
        .map(application_response)
        .collect();
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn find_application_by_id(
    pool: &PgPool,
    application_id: i64,
) -> anyhow::Result<Option<ApplicationResponse>> {
    Ok(application::Entity::find_by_id(application_id)
        .one(&native(pool))
        .await?
        .map(application_response))
}

pub async fn create_application(
    pool: &PgPool,
    request: &CreateApplicationRequest,
) -> anyhow::Result<ApplicationResponse> {
    let scopes = request.default_scopes.clone().unwrap_or_else(all_scopes);
    let model = application::Entity::insert(application::ActiveModel {
        name: sea_orm::Set(request.name.clone()),
        workspace_template: sea_orm::Set(request.workspace_template.clone()),
        default_shell: sea_orm::Set(request.default_shell.clone()),
        default_scopes: sea_orm::Set(scopes),
        max_timeout_ms: sea_orm::Set(request.max_timeout_ms),
        max_output_bytes: sea_orm::Set(request.max_output_bytes),
        max_file_bytes: sea_orm::Set(request.max_file_bytes),
        max_sessions: sea_orm::Set(request.max_sessions),
        network_enabled: sea_orm::Set(request.network_enabled.unwrap_or(true)),
        ..Default::default()
    })
    .exec_with_returning(&native(pool))
    .await?;
    Ok(application_response(model))
}

pub async fn update_application(
    pool: &PgPool,
    application_id: i64,
    request: &UpdateApplicationRequest,
) -> anyhow::Result<Option<ApplicationResponse>> {
    let mut update = application::Entity::update_many()
        .col_expr(application::Column::Name, Expr::val(request.name.clone()))
        .col_expr(application::Column::IsActive, Expr::val(request.is_active))
        .col_expr(
            application::Column::WorkspaceTemplate,
            Expr::val(request.workspace_template.clone()),
        )
        .col_expr(
            application::Column::DefaultShell,
            Expr::val(request.default_shell.clone()),
        )
        .col_expr(
            application::Column::MaxTimeoutMs,
            Expr::val(request.max_timeout_ms),
        )
        .col_expr(
            application::Column::MaxOutputBytes,
            Expr::val(request.max_output_bytes),
        )
        .col_expr(
            application::Column::MaxFileBytes,
            Expr::val(request.max_file_bytes),
        )
        .col_expr(
            application::Column::MaxSessions,
            Expr::val(request.max_sessions),
        )
        .col_expr(
            application::Column::NetworkEnabled,
            Expr::val(request.network_enabled.unwrap_or(true)),
        )
        .col_expr(application::Column::UpdatedAt, Expr::cust("NOW()"));
    if let Some(scopes) = request.default_scopes.clone() {
        update = update.col_expr(application::Column::DefaultScopes, Expr::val(scopes));
    }
    let model = update
        .filter(application::Column::ApplicationId.eq(application_id))
        .exec_with_returning(&native(pool))
        .await?
        .into_iter()
        .next();
    Ok(model.map(application_response))
}

pub async fn list_application_tokens(
    pool: &PgPool,
    params: &ListApplicationTokensParams,
) -> anyhow::Result<Page<ApplicationTokenResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let is_active = params.is_active.or(Some(true));
    let db = native(pool);
    let total = application_token_query(params, is_active)
        .count(&db)
        .await? as i64;
    let items = application_token_query(params, is_active)
        .order_by(application_token::Column::CreatedAt, sea_orm::Order::Desc)
        .order_by(application_token::Column::TokenId, sea_orm::Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(&db)
        .await?
        .into_iter()
        .map(application_token_response)
        .collect();
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn find_application_token_by_id(
    pool: &PgPool,
    token_id: i64,
) -> anyhow::Result<Option<ApplicationTokenResponse>> {
    Ok(application_token::Entity::find_by_id(token_id)
        .one(&native(pool))
        .await?
        .map(application_token_response))
}

pub async fn create_application_token(
    pool: &PgPool,
    request: &CreateApplicationTokenRequest,
) -> anyhow::Result<(String, ApplicationTokenResponse)> {
    let token = generate_token();
    let token_hash = crate::auth::hash_bearer_token(&token);
    let scopes = request.scopes.clone().unwrap_or_else(all_scopes);
    let model = application_token::Entity::insert(application_token::ActiveModel {
        application_id: sea_orm::Set(request.application_id),
        token_name: sea_orm::Set(request.token_name.clone()),
        token_hash: sea_orm::Set(token_hash),
        is_active: sea_orm::Set(true),
        expires_at: sea_orm::Set(request.expires_at),
        scopes: sea_orm::Set(scopes),
        max_timeout_ms: sea_orm::Set(request.max_timeout_ms),
        max_output_bytes: sea_orm::Set(request.max_output_bytes),
        max_file_bytes: sea_orm::Set(request.max_file_bytes),
        max_sessions: sea_orm::Set(request.max_sessions),
        network_enabled: sea_orm::Set(request.network_enabled.unwrap_or(true)),
        ..Default::default()
    })
    .exec_with_returning(&native(pool))
    .await?;
    Ok((token, application_token_response(model)))
}

pub async fn revoke_application_token(pool: &PgPool, token_id: i64) -> anyhow::Result<bool> {
    let result = application_token::Entity::update_many()
        .col_expr(application_token::Column::IsActive, Expr::val(false))
        .filter(application_token::Column::TokenId.eq(token_id))
        .exec(&native(pool))
        .await?;
    Ok(result.rows_affected > 0)
}

fn application_token_update(
    request: &UpdateApplicationTokenRequest,
) -> sea_orm::query::UpdateMany<application_token::Entity> {
    application_token::Entity::update_many()
        .col_expr(
            application_token::Column::ExpiresAt,
            assignment(application_token::Column::ExpiresAt, request.expires_at),
        )
        .col_expr(
            application_token::Column::Scopes,
            assignment(application_token::Column::Scopes, request.scopes.clone()),
        )
        .col_expr(
            application_token::Column::MaxTimeoutMs,
            assignment(
                application_token::Column::MaxTimeoutMs,
                request.max_timeout_ms,
            ),
        )
        .col_expr(
            application_token::Column::MaxOutputBytes,
            assignment(
                application_token::Column::MaxOutputBytes,
                request.max_output_bytes,
            ),
        )
        .col_expr(
            application_token::Column::MaxFileBytes,
            assignment(
                application_token::Column::MaxFileBytes,
                request.max_file_bytes,
            ),
        )
        .col_expr(
            application_token::Column::MaxSessions,
            assignment(application_token::Column::MaxSessions, request.max_sessions),
        )
        .col_expr(
            application_token::Column::NetworkEnabled,
            assignment(
                application_token::Column::NetworkEnabled,
                request.network_enabled,
            ),
        )
}

pub async fn update_application_token(
    pool: &PgPool,
    token_id: i64,
    request: &UpdateApplicationTokenRequest,
) -> anyhow::Result<Option<ApplicationTokenResponse>> {
    let model = application_token_update(request)
        .filter(application_token::Column::TokenId.eq(token_id))
        .exec_with_returning(&native(pool))
        .await?
        .into_iter()
        .next();
    Ok(model.map(application_token_response))
}

fn generate_token() -> String {
    let random = rand::rng()
        .sample_iter(Alphanumeric)
        .take(48)
        .map(char::from)
        .collect::<String>();
    format!("df_{random}")
}

fn all_scopes() -> Vec<String> {
    ALL_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DbBackend, QueryTrait};

    fn empty_mcp_update() -> UpdateMcpTokenRequest {
        UpdateMcpTokenRequest {
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        }
    }

    fn empty_application_update() -> UpdateApplicationTokenRequest {
        UpdateApplicationTokenRequest {
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        }
    }

    #[test]
    fn empty_mcp_token_update_is_a_single_self_assigning_update() {
        let sql = mcp_token_update(&empty_mcp_update())
            .filter(mcp_token::Column::TokenId.eq(7))
            .build(DbBackend::Postgres)
            .sql;
        assert!(sql.starts_with("UPDATE \"mcp_tokens\" SET"), "{sql}");
        assert!(!sql.contains("SELECT"), "{sql}");
        assert!(
            sql.contains("\"expires_at\" = \"mcp_tokens\".\"expires_at\""),
            "{sql}"
        );
        assert!(
            sql.contains("\"scopes\" = \"mcp_tokens\".\"scopes\""),
            "{sql}"
        );
        assert!(
            sql.contains("\"network_enabled\" = \"mcp_tokens\".\"network_enabled\""),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE \"mcp_tokens\".\"token_id\" = "),
            "{sql}"
        );
    }

    #[test]
    fn empty_application_token_update_is_a_single_self_assigning_update() {
        let sql = application_token_update(&empty_application_update())
            .filter(application_token::Column::TokenId.eq(7))
            .build(DbBackend::Postgres)
            .sql;
        assert!(
            sql.starts_with("UPDATE \"application_tokens\" SET"),
            "{sql}"
        );
        assert!(!sql.contains("SELECT"), "{sql}");
        assert!(
            sql.contains("\"expires_at\" = \"application_tokens\".\"expires_at\""),
            "{sql}"
        );
        assert!(
            sql.contains("\"scopes\" = \"application_tokens\".\"scopes\""),
            "{sql}"
        );
        assert!(
            sql.contains("\"network_enabled\" = \"application_tokens\".\"network_enabled\""),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE \"application_tokens\".\"token_id\" = "),
            "{sql}"
        );
    }
}
