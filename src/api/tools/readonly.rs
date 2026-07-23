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
    tools::{
        params::{GlobParams, GrepParams, ReadParams, StatPathParams},
        readonly::types::{GlobOutput, GrepOutput, ReadOutput, StatPathOutput},
        shared,
    },
};

use super::{admin_target_actor, map_tool_error, self_service_actor};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tools/read", post(read))
        .route("/api/admin/users/{user_id}/tools/read", post(read_as_admin))
        .route("/api/tools/glob", post(glob))
        .route("/api/admin/users/{user_id}/tools/glob", post(glob_as_admin))
        .route("/api/tools/grep", post(grep))
        .route("/api/admin/users/{user_id}/tools/grep", post(grep_as_admin))
        .route("/api/tools/stat", post(stat))
        .route("/api/admin/users/{user_id}/tools/stat", post(stat_as_admin))
}

#[utoipa::path(
    post,
    path = "/api/tools/read",
    tag = "tools",
    request_body = ReadParams,
    responses(
        (status = 200, body = ReadOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn read(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<ReadParams>,
) -> Result<Json<ReadOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    Ok(Json(
        shared::read(&state, &actor, &params).map_err(map_tool_error)?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/read",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = ReadParams,
    responses(
        (status = 200, body = ReadOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn read_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<ReadParams>,
) -> Result<Json<ReadOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    Ok(Json(
        shared::read(&state, &actor, &params).map_err(map_tool_error)?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/tools/glob",
    tag = "tools",
    request_body = GlobParams,
    responses(
        (status = 200, body = GlobOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn glob(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<GlobParams>,
) -> Result<Json<GlobOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    Ok(Json(
        shared::glob(&state, &actor, &params)
            .await
            .map_err(map_tool_error)?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/glob",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = GlobParams,
    responses(
        (status = 200, body = GlobOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn glob_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<GlobParams>,
) -> Result<Json<GlobOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    Ok(Json(
        shared::glob(&state, &actor, &params)
            .await
            .map_err(map_tool_error)?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/tools/grep",
    tag = "tools",
    request_body = GrepParams,
    responses(
        (status = 200, body = GrepOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn grep(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<GrepParams>,
) -> Result<Json<GrepOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    let output = shared::grep(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/grep",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = GrepParams,
    responses(
        (status = 200, body = GrepOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn grep_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<GrepParams>,
) -> Result<Json<GrepOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    let output = shared::grep(&state, &actor, &params)
        .await
        .map_err(map_tool_error)?;
    Ok(Json(output))
}

#[utoipa::path(
    post,
    path = "/api/tools/stat",
    tag = "tools",
    request_body = StatPathParams,
    responses(
        (status = 200, body = StatPathOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn stat(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<StatPathParams>,
) -> Result<Json<StatPathOutput>, AppError> {
    let actor = self_service_actor(&state, &jar, &headers).await?;
    Ok(Json(
        shared::stat_path(&state, &actor, &params).map_err(map_tool_error)?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/users/{user_id}/tools/stat",
    tag = "admin-tools",
    params(("user_id" = i64, Path, description = "Target user identifier")),
    request_body = StatPathParams,
    responses(
        (status = 200, body = StatPathOutput),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn stat_as_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    ValidatedJson(params): ValidatedJson<StatPathParams>,
) -> Result<Json<StatPathOutput>, AppError> {
    let actor = admin_target_actor(&state, &jar, &headers, user_id).await?;
    Ok(Json(
        shared::stat_path(&state, &actor, &params).map_err(map_tool_error)?,
    ))
}
