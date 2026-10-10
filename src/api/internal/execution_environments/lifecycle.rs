//! Authenticated asynchronous execution environment lifecycle handlers
//! (issue 742 P3).
//!
//! `provision`, `stop` and `replace` admit one durable operation intent and
//! return the lifecycle descriptor; `status` reconciles persisted evidence.
//! Every route resolves the application identity from the bearer first (never
//! from the body), then runs a short owner-scoped transaction. No state volume,
//! token, manager or host endpoint is exposed, and no Docker work happens here.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use uuid::Uuid;

use crate::{
    AppState,
    api::{internal::shared::application_actor, validation::ValidatedJson},
    db::execution_environments::{
        EnvironmentOperationDescriptor, LifecycleError,
        execution_environment_status as lifecycle_status,
        provision_execution_environment as lifecycle_provision,
        replace_execution_environment as lifecycle_replace,
        stop_execution_environment as lifecycle_stop,
    },
    error::AppError,
};

use super::types::{
    ProvisionExecutionEnvironmentRequest, ReplaceExecutionEnvironmentRequest,
    StopExecutionEnvironmentRequest,
};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/provision",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier")
    ),
    request_body = ProvisionExecutionEnvironmentRequest,
    responses(
        (status = 200, body = EnvironmentOperationDescriptor),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn provision_execution_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(environment_id): Path<Uuid>,
    ValidatedJson(request): ValidatedJson<ProvisionExecutionEnvironmentRequest>,
) -> Result<Json<EnvironmentOperationDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = lifecycle_provision(
        &state.db,
        application_id,
        environment_id,
        request.expected_generation,
        &request.operation_token,
    )
    .await
    .map_err(map_lifecycle_error)?;
    Ok(Json(descriptor))
}

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/stop",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier")
    ),
    request_body = StopExecutionEnvironmentRequest,
    responses(
        (status = 200, body = EnvironmentOperationDescriptor),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn stop_execution_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(environment_id): Path<Uuid>,
    ValidatedJson(request): ValidatedJson<StopExecutionEnvironmentRequest>,
) -> Result<Json<EnvironmentOperationDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = lifecycle_stop(
        &state.db,
        application_id,
        environment_id,
        request.expected_generation,
        &request.operation_token,
    )
    .await
    .map_err(map_lifecycle_error)?;
    Ok(Json(descriptor))
}

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/replace",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier")
    ),
    request_body = ReplaceExecutionEnvironmentRequest,
    responses(
        (status = 200, body = EnvironmentOperationDescriptor),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn replace_execution_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(environment_id): Path<Uuid>,
    ValidatedJson(request): ValidatedJson<ReplaceExecutionEnvironmentRequest>,
) -> Result<Json<EnvironmentOperationDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = lifecycle_replace(
        &state.db,
        application_id,
        environment_id,
        request.expected_generation,
        request.expected_container_generation,
        request.expected_stop_job_id,
        &request.operation_token,
    )
    .await
    .map_err(map_lifecycle_error)?;
    Ok(Json(descriptor))
}

#[utoipa::path(
    get,
    path = "/api/internal/execution-environments/{environment_id}/status",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier")
    ),
    responses(
        (status = 200, body = EnvironmentOperationDescriptor),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn execution_environment_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(environment_id): Path<Uuid>,
) -> Result<Json<EnvironmentOperationDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = lifecycle_status(&state.db, application_id, environment_id)
        .await
        .map_err(map_lifecycle_error)?;
    Ok(Json(descriptor))
}

/// Map a typed lifecycle refusal to a stable HTTP status. No variant exposes a
/// secret, manager detail, host path or token.
pub(super) fn map_lifecycle_error(error: LifecycleError) -> AppError {
    match error {
        LifecycleError::NotFound => AppError::not_found("execution environment not found"),
        LifecycleError::StaleGeneration { .. } => {
            AppError::conflict("expected generation does not match the stored generation")
        }
        LifecycleError::StaleCandidate { .. } => {
            AppError::conflict("expected container generation does not match the stored candidate")
        }
        LifecycleError::ProofMismatch => {
            AppError::conflict("stop proof does not match the stored lifecycle evidence")
        }
        LifecycleError::TokenConflict => {
            AppError::conflict("operation token was already used with a different intent")
        }
        LifecycleError::InvalidToken => {
            AppError::bad_request("operation token must be non-blank and bounded")
        }
        LifecycleError::NotProvisioned => {
            AppError::conflict("execution environment has no provision candidate to stop")
        }
        LifecycleError::AlreadyProvisioned => {
            AppError::conflict("execution environment already has a provision candidate")
        }
        LifecycleError::StopInFlight => {
            AppError::conflict("a stop operation is already in flight for this environment")
        }
        LifecycleError::UnresolvedBlockers => {
            AppError::conflict("execution environment has unresolved lifecycle blockers")
        }
        LifecycleError::ManagerUnavailable => {
            AppError::service_unavailable("no runner manager is available to provision")
        }
        LifecycleError::Internal(error) => AppError::internal(error),
    }
}
