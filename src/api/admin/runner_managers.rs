use axum::{
    Json,
    extract::{Path, State},
};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    AppState,
    api::validation::ValidatedJson,
    db::types::{
        CreateRunnerManagerRequest, CreateRunnerManagerResponse, RunnerManagerResponse,
        UpdateRunnerManagerRequest,
    },
    error::AppError,
};

use super::{shared::record_admin_audit, users::require_admin};

#[utoipa::path(
    get,
    path = "/api/admin/runner-managers",
    tag = "admin-operations",
    responses((status = 200, body = [RunnerManagerResponse]))
)]
pub async fn list_runner_managers(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Vec<RunnerManagerResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    Ok(Json(
        crate::db::queries::list_runner_managers(&state.db).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/runner-managers",
    tag = "admin-operations",
    request_body = CreateRunnerManagerRequest,
    responses((status = 200, body = CreateRunnerManagerResponse))
)]
pub async fn create_runner_manager(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(request): ValidatedJson<CreateRunnerManagerRequest>,
) -> Result<Json<CreateRunnerManagerResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let (manager, token) = crate::db::queries::create_runner_manager(&state.db, &request).await?;
    record_admin_audit(
        &state,
        &admin,
        "admin.runner_manager.create",
        "runner_manager",
        manager.runner_manager_id.to_string(),
        serde_json::json!({ "name": manager.name, "endpoint": manager.endpoint }),
    )
    .await?;
    Ok(Json(CreateRunnerManagerResponse { manager, token }))
}

#[utoipa::path(
    patch,
    path = "/api/admin/runner-managers/{runner_manager_id}",
    tag = "admin-operations",
    params(("runner_manager_id" = i64, Path, description = "Runner manager id")),
    request_body = UpdateRunnerManagerRequest,
    responses((status = 200, body = RunnerManagerResponse))
)]
pub async fn update_runner_manager(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(runner_manager_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<UpdateRunnerManagerRequest>,
) -> Result<Json<RunnerManagerResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let manager = crate::db::queries::update_runner_manager(&state.db, runner_manager_id, &request)
        .await?
        .ok_or_else(|| AppError::not_found("runner manager not found"))?;
    record_admin_audit(
        &state,
        &admin,
        "admin.runner_manager.update",
        "runner_manager",
        runner_manager_id.to_string(),
        serde_json::json!({ "enabled": manager.enabled, "image": manager.image }),
    )
    .await?;
    Ok(Json(manager))
}
