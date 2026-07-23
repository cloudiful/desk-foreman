use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    AppState,
    api::validation::ValidatedJson,
    error::AppError,
    shell::ShellToolOutput,
    tools::{
        params::{ApplyPatchParams, CancelSessionParams, ShellParams, WriteStdinParams},
        shared::{self, ApplyPatchOutput},
    },
};

use super::{admin_target_actor, map_tool_error, self_service_actor};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tools/shell", post(shell))
        .route(
            "/api/admin/users/{user_id}/tools/shell",
            post(shell_as_admin),
        )
        .route("/api/tools/write-stdin", post(write_stdin))
        .route(
            "/api/admin/users/{user_id}/tools/write-stdin",
            post(write_stdin_as_admin),
        )
        .route("/api/tools/cancel-session", post(cancel_session))
        .route(
            "/api/admin/users/{user_id}/tools/cancel-session",
            post(cancel_session_as_admin),
        )
        .route("/api/tools/apply-patch", post(apply_patch))
        .route(
            "/api/admin/users/{user_id}/tools/apply-patch",
            post(apply_patch_as_admin),
        )
}

#[utoipa::path(
    post,
    path = "/api/tools/shell",
    tag = "tools",
    request_body = ShellParams,
    responses(
        (status = 200, body = ShellToolOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn shell(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<ShellParams>,
) -> Result<Json<ShellToolOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    let output = shared::shell(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/shell",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = ShellParams,
    responses(
        (status = 200, body = ShellToolOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn shell_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<ShellParams>,
) -> Result<Json<ShellToolOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    let output = shared::shell(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/tools/write-stdin",
    tag = "tools",
    request_body = WriteStdinParams,
    responses(
        (status = 200, body = ShellToolOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn write_stdin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<WriteStdinParams>,
) -> Result<Json<ShellToolOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    let output = shared::write_stdin(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/write-stdin",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = WriteStdinParams,
    responses(
        (status = 200, body = ShellToolOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn write_stdin_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<WriteStdinParams>,
) -> Result<Json<ShellToolOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    let output = shared::write_stdin(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/tools/cancel-session",
    tag = "tools",
    request_body = CancelSessionParams,
    responses(
        (status = 200, body = shared::CancelSessionOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn cancel_session(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<CancelSessionParams>,
) -> Result<Json<shared::CancelSessionOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    let output = shared::cancel_session(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/cancel-session",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = CancelSessionParams,
    responses(
        (status = 200, body = shared::CancelSessionOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn cancel_session_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<CancelSessionParams>,
) -> Result<Json<shared::CancelSessionOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    let output = shared::cancel_session(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/tools/apply-patch",
    tag = "tools",
    request_body = ApplyPatchParams,
    responses(
        (status = 200, body = ApplyPatchOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn apply_patch(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<ApplyPatchParams>,
) -> Result<Json<ApplyPatchOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    let output = shared::apply_patch(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/apply-patch",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = ApplyPatchParams,
    responses(
        (status = 200, body = ApplyPatchOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn apply_patch_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<ApplyPatchParams>,
) -> Result<Json<ApplyPatchOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    let output = shared::apply_patch(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}
