use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::{
    AppState,
    api::validation::{ValidatedJson, ValidatedQuery},
    db::types::{
        CreateMcpTokenRequest, CreateMcpTokenResponse, ListMcpTokensParams, McpTokenResponse, Page,
        UpdateMcpTokenRequest,
    },
    error::AppError,
};

use super::shared::record_admin_audit;
use super::users::require_admin;

#[utoipa::path(
    get,
    path = "/api/admin/mcp-tokens",
    tag = "admin-users",
    params(ListMcpTokensParams),
    responses(
        (status = 200, body = Page<McpTokenResponse>),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn list_mcp_tokens(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedQuery(params): ValidatedQuery<ListMcpTokensParams>,
) -> Result<Json<Page<McpTokenResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    Ok(Json(
        crate::db::queries::list_mcp_tokens(&state.db, &params).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/mcp-tokens",
    tag = "admin-users",
    request_body = CreateMcpTokenRequest,
    responses(
        (status = 201, body = CreateMcpTokenResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn create_mcp_token(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(request): ValidatedJson<CreateMcpTokenRequest>,
) -> Result<(StatusCode, Json<CreateMcpTokenResponse>), AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(_) = crate::db::queries::find_user_by_id(&state.db, request.user_id).await? else {
        return Err(AppError::not_found("user not found"));
    };
    let (token, metadata) = crate::db::queries::create_mcp_token(&state.db, &request).await?;
    record_admin_audit(
        &state,
        &admin,
        "admin.mcp_token.create",
        "mcp_token",
        metadata.token_id.to_string(),
        json!({ "user_id": metadata.user_id, "token_name": metadata.token_name }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateMcpTokenResponse { token, metadata }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/admin/mcp-tokens/{token_id}",
    tag = "admin-users",
    params(
        ("token_id" = i64, Path, description = "MCP token identifier")
    ),
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn delete_mcp_token(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(token_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(token) = crate::db::queries::find_mcp_token_by_id(&state.db, token_id).await? else {
        return Err(AppError::not_found("mcp token not found"));
    };
    if !crate::db::queries::revoke_mcp_token(&state.db, token_id).await? {
        return Err(AppError::not_found("mcp token not found"));
    }
    record_admin_audit(
        &state,
        &admin,
        "admin.mcp_token.revoke",
        "mcp_token",
        token_id.to_string(),
        json!({ "user_id": token.user_id, "token_name": token.token_name }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch,
    path = "/api/admin/mcp-tokens/{token_id}",
    tag = "admin-users",
    params(("token_id" = i64, Path, description = "MCP token identifier")),
    request_body = UpdateMcpTokenRequest,
    responses((status = 200, body = McpTokenResponse))
)]
pub async fn update_mcp_token(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(token_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<UpdateMcpTokenRequest>,
) -> Result<Json<McpTokenResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(token) = crate::db::queries::update_mcp_token(&state.db, token_id, &request).await?
    else {
        return Err(AppError::not_found("mcp token not found"));
    };
    record_admin_audit(
        &state,
        &admin,
        "admin.mcp_token.update",
        "mcp_token",
        token_id.to_string(),
        json!({
            "scopes": token.scopes, "expires_at": token.expires_at
        }),
    )
    .await?;
    Ok(Json(token))
}
