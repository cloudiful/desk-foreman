//! Internal runner-manager endpoints.
//!
//! These endpoints accept a runner-manager bearer token (issued by the
//! admin runner-manager create endpoint) and let the runner-manager fetch
//! its configuration, poll for jobs, report completions, and report
//! runner lifecycle events.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use runner_protocol::{
    RUNNER_JOB_POLL_TIMEOUT_SECS, RunnerJob, RunnerJobResult, RunnerLifecycleEvent,
};

use crate::{AppState, error::AppError};

pub(super) async fn runner_manager_from_token(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<crate::db::types::RunnerManagerRecord, AppError> {
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| AppError::unauthorized("runner manager token required"))?
        .to_str()
        .map_err(|_| AppError::unauthorized("invalid runner manager token"))?;
    let token = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| AppError::unauthorized("invalid runner manager token"))?;
    let manager = crate::db::queries::find_runner_manager_by_token(&state.db, token)
        .await?
        .ok_or_else(|| AppError::not_found("runner manager is not registered"))?;
    if !manager.enabled {
        return Err(AppError::forbidden("runner manager is disabled"));
    }
    crate::db::queries::touch_runner_manager(&state.db, manager.runner_manager_id).await?;
    Ok(manager)
}

#[utoipa::path(
    get,
    path = "/api/internal/runner-manager/config",
    tag = "internal",
    responses(
        (status = 200, body = crate::db::types::RunnerManagerResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn runner_manager_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::db::types::RunnerManagerResponse>, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    Ok(Json(crate::db::types::RunnerManagerResponse {
        runner_manager_id: manager.runner_manager_id,
        name: manager.name,
        endpoint: manager.endpoint,
        enabled: manager.enabled,
        image: manager.image,
        network_enabled: manager.network_enabled,
        max_output_bytes: manager.max_output_bytes,
        max_timeout_ms: manager.max_timeout_ms,
        max_sessions: manager.max_sessions,
        pids_limit: manager.pids_limit,
        memory_limit: manager.memory_limit,
        cpu_limit: manager.cpu_limit,
        host_workspace_root: manager.host_workspace_root,
        status: manager.status,
        last_seen_at: manager.last_seen_at,
        created_at: manager.created_at,
        updated_at: manager.updated_at,
    }))
}

#[utoipa::path(
    get,
    path = "/api/internal/runner-manager/jobs/next",
    tag = "internal",
    responses((status = 200, body = Option<runner_protocol::RunnerJob>))
)]
pub async fn next_runner_job(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Option<RunnerJob>>, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    let job = tokio::time::timeout(
        std::time::Duration::from_secs(RUNNER_JOB_POLL_TIMEOUT_SECS),
        state.runner_broker.next_job(manager.runner_manager_id),
    )
    .await
    .unwrap_or(None);
    Ok(Json(job))
}

#[utoipa::path(
    post,
    path = "/api/internal/runner-manager/jobs/result",
    tag = "internal",
    request_body = runner_protocol::RunnerJobResult,
    responses((status = 204))
)]
pub async fn complete_runner_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(result): Json<RunnerJobResult>,
) -> Result<StatusCode, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    state
        .runner_broker
        .complete_job(manager.runner_manager_id, result)
        .await
        .map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/internal/runner-manager/workspace-runners/report",
    tag = "internal",
    request_body = Vec<RunnerLifecycleEvent>,
    responses((status = 204))
)]
pub async fn report_workspace_runner(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(events): Json<Vec<RunnerLifecycleEvent>>,
) -> Result<StatusCode, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    if events.len() > 256 {
        return Err(AppError::bad_request("too many runner lifecycle events"));
    }
    for event in &events {
        if event.container_name != event.owner.container_name() {
            return Err(AppError::bad_request(
                "runner container does not match owner",
            ));
        }
        crate::db::queries::report_workspace_runner(&state.db, manager.runner_manager_id, event)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
