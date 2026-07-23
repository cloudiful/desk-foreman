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
    auth,
    db::types::{
        CreateUserRequest, ListUsersParams, ResetPasswordRequest, UpdateUserRequest,
        UserPageResponse, UserResponse,
    },
    error::AppError,
    workspace::default_user_workspace,
};

use super::shared::{map_db_conflict, record_admin_audit};

pub use super::shared::require_admin;

#[utoipa::path(
    get,
    path = "/api/admin/users",
    tag = "admin-users",
    params(ListUsersParams),
    responses(
        (status = 200, body = UserPageResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn list_users(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedQuery(params): ValidatedQuery<ListUsersParams>,
) -> Result<Json<UserPageResponse>, AppError> {
    require_admin(&state, &jar).await?;
    let (items, total) = crate::db::queries::list_users(&state.db, &params)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(UserPageResponse {
        items,
        total,
        limit: params.limit.unwrap_or(20).clamp(1, 100),
        offset: params.offset.unwrap_or(0).max(0),
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/users",
    tag = "admin-users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, body = UserResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn create_user(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(request): ValidatedJson<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), AppError> {
    let admin = require_admin(&state, &jar).await?;

    let password_hash = auth::hash_password(&request.password)?;
    let user = crate::db::queries::create_user(&state.db, &request, &password_hash)
        .await
        .map_err(map_db_conflict)?;
    let user = if user.workspace_root.is_empty() {
        let workspace_root = default_user_workspace(&state.config.workspace_root, user.user_id)
            .to_string_lossy()
            .to_string();
        crate::db::queries::update_user_workspace(&state.db, user.user_id, &workspace_root)
            .await?
            .ok_or_else(|| AppError::internal(anyhow::anyhow!("user disappeared after creation")))?
    } else {
        user
    };
    record_admin_audit(
        &state,
        &admin,
        "admin.user.create",
        "user",
        user.user_id.to_string(),
        json!({ "login_name": user.login_name }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(user)))
}

#[utoipa::path(
    patch,
    path = "/api/admin/users/{user_id}",
    tag = "admin-users",
    request_body = UpdateUserRequest,
    params(
        ("user_id" = i64, Path, description = "User identifier")
    ),
    responses(
        (status = 200, body = UserResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn update_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(user_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<UpdateUserRequest>,
) -> Result<Json<UserResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(user) = crate::db::queries::update_user(&state.db, user_id, &request)
        .await
        .map_err(map_db_conflict)?
    else {
        return Err(AppError::not_found("user not found"));
    };
    if !user.is_active {
        crate::db::queries::revoke_user_sessions(&state.db, user.user_id).await?;
    }
    record_admin_audit(
        &state,
        &admin,
        "admin.user.update",
        "user",
        user.user_id.to_string(),
        json!({ "is_active": user.is_active, "is_admin": user.is_admin }),
    )
    .await?;
    Ok(Json(user))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/reset-password",
    tag = "admin-users",
    request_body = ResetPasswordRequest,
    params(
        ("user_id" = i64, Path, description = "User identifier")
    ),
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn reset_user_password(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(user_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<ResetPasswordRequest>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let password_hash = auth::hash_password(&request.password)?;
    if !crate::db::queries::reset_user_password(&state.db, user_id, &password_hash).await? {
        return Err(AppError::not_found("user not found"));
    }
    crate::db::queries::revoke_user_sessions(&state.db, user_id).await?;
    record_admin_audit(
        &state,
        &admin,
        "admin.user.reset_password",
        "user",
        user_id.to_string(),
        json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/admin/users/{user_id}",
    tag = "admin-users",
    params(
        ("user_id" = i64, Path, description = "User identifier")
    ),
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn delete_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(user_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &jar).await?;
    if !crate::db::queries::deactivate_user(&state.db, user_id).await? {
        return Err(AppError::not_found("user not found"));
    }
    crate::db::queries::revoke_user_sessions(&state.db, user_id).await?;
    record_admin_audit(
        &state,
        &admin,
        "admin.user.deactivate",
        "user",
        user_id.to_string(),
        json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
