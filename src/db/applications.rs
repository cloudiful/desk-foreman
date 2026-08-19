use anyhow::Context;
use rand::{RngExt, distr::Alphanumeric};
use sqlx::PgPool;

use crate::db::types::{
    ApplicationResponse, ApplicationTokenResponse, CreateApplicationRequest,
    CreateApplicationTokenRequest, CreateMcpTokenRequest, ListApplicationsParams,
    ListApplicationTokensParams, ListMcpTokensParams, McpTokenResponse, Page,
    UpdateApplicationRequest, UpdateApplicationTokenRequest, UpdateMcpTokenRequest,
};
use crate::policy::ALL_SCOPES;
use crate::secrets::EncryptedSecret;

pub async fn list_mcp_tokens(
    pool: &PgPool,
    params: &ListMcpTokensParams,
) -> anyhow::Result<Page<McpTokenResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let is_active = params.is_active.or(Some(true));
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_mcp_tokens.sql"))
        .bind(&params.search)
        .bind(params.user_id)
        .bind(is_active)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, McpTokenResponse>(include_str!("../sql/list_mcp_tokens.sql"))
        .bind(&params.search)
        .bind(params.user_id)
        .bind(is_active)
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

pub async fn find_mcp_token_by_id(
    pool: &PgPool,
    token_id: i64,
) -> anyhow::Result<Option<McpTokenResponse>> {
    sqlx::query_as::<_, McpTokenResponse>(include_str!("../sql/find_mcp_token_by_id.sql"))
        .bind(token_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn create_mcp_token(
    pool: &PgPool,
    request: &CreateMcpTokenRequest,
) -> anyhow::Result<(String, McpTokenResponse)> {
    let token = generate_token();
    let token_hash = crate::auth::hash_bearer_token(&token);
    let scopes = request.scopes.clone().unwrap_or_else(all_scopes);
    let row = sqlx::query_as::<_, McpTokenResponse>(include_str!("../sql/create_mcp_token.sql"))
        .bind(&request.token_name)
        .bind(&token_hash)
        .bind(request.user_id)
        .bind(request.expires_at)
        .bind(scopes)
        .bind(request.max_timeout_ms)
        .bind(request.max_output_bytes)
        .bind(request.max_file_bytes)
        .bind(request.max_sessions)
        .bind(request.network_enabled.unwrap_or(true))
        .fetch_one(pool)
        .await?;
    Ok((token, row))
}

pub async fn revoke_mcp_token(pool: &PgPool, token_id: i64) -> anyhow::Result<bool> {
    let result = sqlx::query(include_str!("../sql/revoke_mcp_token.sql"))
        .bind(token_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_mcp_token(
    pool: &PgPool,
    token_id: i64,
    request: &UpdateMcpTokenRequest,
) -> anyhow::Result<Option<McpTokenResponse>> {
    sqlx::query_as::<_, McpTokenResponse>(include_str!("../sql/update_mcp_token.sql"))
        .bind(token_id)
        .bind(request.expires_at)
        .bind(&request.scopes)
        .bind(request.max_timeout_ms)
        .bind(request.max_output_bytes)
        .bind(request.max_file_bytes)
        .bind(request.max_sessions)
        .bind(request.network_enabled)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn list_applications(
    pool: &PgPool,
    params: &ListApplicationsParams,
) -> anyhow::Result<Page<ApplicationResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_applications.sql"))
        .bind(&params.search)
        .bind(params.is_active)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, ApplicationResponse>(include_str!("../sql/list_applications.sql"))
        .bind(&params.search)
        .bind(params.is_active)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .context("failed to list applications")?;
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
    sqlx::query_as::<_, ApplicationResponse>(include_str!("../sql/find_application_by_id.sql"))
        .bind(application_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn create_application(
    pool: &PgPool,
    request: &CreateApplicationRequest,
    secret: Option<&EncryptedSecret>,
) -> anyhow::Result<ApplicationResponse> {
    sqlx::query_as::<_, ApplicationResponse>(include_str!("../sql/create_application.sql"))
        .bind(&request.name)
        .bind(&request.workspace_template)
        .bind(&request.default_shell)
        .bind(request.default_scopes.clone().unwrap_or_else(all_scopes))
        .bind(request.max_timeout_ms)
        .bind(request.max_output_bytes)
        .bind(request.max_file_bytes)
        .bind(request.max_sessions)
        .bind(request.network_enabled.unwrap_or(true))
        .bind(request.approval_mode.as_deref())
        .bind(&request.approval_endpoint)
        .bind(&request.approval_model)
        .bind(request.approval_timeout_ms)
        .bind(request.approval_max_input_bytes)
        .bind(request.approval_max_concurrent)
        .bind(request.approval_max_output_tokens)
        .bind(secret.map(|value| value.ciphertext.clone()))
        .bind(secret.map(|value| value.nonce.clone()))
        .bind(secret.map(|value| value.key_version))
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn update_application(
    pool: &PgPool,
    application_id: i64,
    request: &UpdateApplicationRequest,
    secret: Option<&EncryptedSecret>,
) -> anyhow::Result<Option<ApplicationResponse>> {
    sqlx::query_as::<_, ApplicationResponse>(include_str!("../sql/update_application.sql"))
        .bind(application_id)
        .bind(&request.name)
        .bind(request.is_active)
        .bind(&request.workspace_template)
        .bind(&request.default_shell)
        .bind(request.default_scopes.clone())
        .bind(request.max_timeout_ms)
        .bind(request.max_output_bytes)
        .bind(request.max_file_bytes)
        .bind(request.max_sessions)
        .bind(request.network_enabled.unwrap_or(true))
        .bind(&request.approval_mode)
        .bind(&request.approval_endpoint)
        .bind(&request.approval_model)
        .bind(request.approval_timeout_ms)
        .bind(request.approval_max_input_bytes)
        .bind(request.approval_max_concurrent)
        .bind(request.approval_max_output_tokens)
        .bind(secret.map(|value| value.ciphertext.clone()))
        .bind(secret.map(|value| value.nonce.clone()))
        .bind(secret.map(|value| value.key_version))
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn list_application_tokens(
    pool: &PgPool,
    params: &ListApplicationTokensParams,
) -> anyhow::Result<Page<ApplicationTokenResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let is_active = params.is_active.or(Some(true));
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_application_tokens.sql"))
        .bind(params.application_id)
        .bind(is_active)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, ApplicationTokenResponse>(include_str!(
        "../sql/list_application_tokens.sql"
    ))
    .bind(params.application_id)
    .bind(is_active)
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

pub async fn find_application_token_by_id(
    pool: &PgPool,
    token_id: i64,
) -> anyhow::Result<Option<ApplicationTokenResponse>> {
    sqlx::query_as::<_, ApplicationTokenResponse>(include_str!(
        "../sql/find_application_token_by_id.sql"
    ))
    .bind(token_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn create_application_token(
    pool: &PgPool,
    request: &CreateApplicationTokenRequest,
) -> anyhow::Result<(String, ApplicationTokenResponse)> {
    let token = generate_token();
    let token_hash = crate::auth::hash_bearer_token(&token);
    let scopes = request.scopes.clone().unwrap_or_else(all_scopes);
    let row = sqlx::query_as::<_, ApplicationTokenResponse>(include_str!(
        "../sql/create_application_token.sql"
    ))
    .bind(request.application_id)
    .bind(&request.token_name)
    .bind(&token_hash)
    .bind(request.expires_at)
    .bind(scopes)
    .bind(request.max_timeout_ms)
    .bind(request.max_output_bytes)
    .bind(request.max_file_bytes)
    .bind(request.max_sessions)
    .bind(request.network_enabled.unwrap_or(true))
    .fetch_one(pool)
    .await?;
    Ok((token, row))
}

pub async fn revoke_application_token(pool: &PgPool, token_id: i64) -> anyhow::Result<bool> {
    let result = sqlx::query(include_str!("../sql/revoke_application_token.sql"))
        .bind(token_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_application_token(
    pool: &PgPool,
    token_id: i64,
    request: &UpdateApplicationTokenRequest,
) -> anyhow::Result<Option<ApplicationTokenResponse>> {
    sqlx::query_as::<_, ApplicationTokenResponse>(include_str!(
        "../sql/update_application_token.sql"
    ))
    .bind(token_id)
    .bind(request.expires_at)
    .bind(&request.scopes)
    .bind(request.max_timeout_ms)
    .bind(request.max_output_bytes)
    .bind(request.max_file_bytes)
    .bind(request.max_sessions)
    .bind(request.network_enabled)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
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
