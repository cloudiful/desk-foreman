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
    api::{
        internal::shared::{application_actor, application_scope_auth},
        validation::ValidatedJson,
    },
    db::execution_environments::{
        EnvironmentOperationDescriptor, LifecycleError, ReplaceRequest, ScopeAuthority, ScopedCall,
        execution_environment_status as lifecycle_status,
        provision_execution_environment as lifecycle_provision,
        provision_execution_environment_scoped as lifecycle_provision_scoped,
        replace_execution_environment as lifecycle_replace,
        replace_execution_environment_scoped as lifecycle_replace_scoped,
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
    let stored =
        crate::db::queries::find_execution_environment(&state.db, application_id, environment_id)
            .await
            .map_err(AppError::internal)?;
    // The routing read chooses the branch only; the scoped sibling re-reads the
    // stored scope and revalidates the locked binding/lease before any intent.
    let descriptor = match stored {
        Some(environment) if environment.workspace_binding_id.is_some() => {
            let auth = application_scope_auth(&state, &headers).await?;
            lifecycle_provision_scoped(
                &state.db,
                &state.task_runtime,
                auth.application_id,
                environment_id,
                request.expected_generation,
                &request.operation_token,
                Some(ScopedCall {
                    authority: ScopeAuthority {
                        workspace_binding_id: auth.workspace_binding.workspace_binding_id,
                        lease_owner: auth.lease_owner,
                    },
                    runner: state.runner.as_ref(),
                }),
            )
            .await
        }
        _ => {
            lifecycle_provision(
                &state.db,
                &state.task_runtime,
                application_id,
                environment_id,
                request.expected_generation,
                &request.operation_token,
            )
            .await
        }
    }
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
    let (descriptor, cancel_through_generation) = lifecycle_stop(
        &state.db,
        &state.task_runtime,
        application_id,
        environment_id,
        request.expected_generation,
        &request.operation_token,
    )
    .await
    .map_err(map_lifecycle_error)?;
    if let Some(generation) = cancel_through_generation {
        state
            .runner_broker
            .service_relay()
            .cancel_environment_through(environment_id, generation);
    }
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
    let stored =
        crate::db::queries::find_execution_environment(&state.db, application_id, environment_id)
            .await
            .map_err(AppError::internal)?;
    let descriptor = match stored {
        Some(environment) if environment.workspace_binding_id.is_some() => {
            let auth = application_scope_auth(&state, &headers).await?;
            lifecycle_replace_scoped(
                &state.db,
                &state.task_runtime,
                auth.application_id,
                environment_id,
                &ReplaceRequest {
                    expected_generation: request.expected_generation,
                    expected_container_generation: request.expected_container_generation,
                    expected_stop_job_id: request.expected_stop_job_id,
                    operation_token: &request.operation_token,
                },
                Some(ScopedCall {
                    authority: ScopeAuthority {
                        workspace_binding_id: auth.workspace_binding.workspace_binding_id,
                        lease_owner: auth.lease_owner,
                    },
                    runner: state.runner.as_ref(),
                }),
            )
            .await
        }
        _ => {
            lifecycle_replace(
                &state.db,
                &state.task_runtime,
                application_id,
                environment_id,
                &ReplaceRequest {
                    expected_generation: request.expected_generation,
                    expected_container_generation: request.expected_container_generation,
                    expected_stop_job_id: request.expected_stop_job_id,
                    operation_token: &request.operation_token,
                },
            )
            .await
        }
    }
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
    let descriptor = lifecycle_status(
        &state.db,
        &state.task_runtime,
        application_id,
        environment_id,
    )
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
        LifecycleError::NativeTaskActive => {
            AppError::conflict("execution environment is owned by an active native task")
        }
        LifecycleError::ManagerUnavailable => {
            AppError::service_unavailable("no runner manager is available to provision")
        }
        LifecycleError::ScopeAuthorityRequired => AppError::conflict(
            "workspace-scoped environment requires an authenticated workspace scope",
        ),
        LifecycleError::ScopeIneligible => AppError::conflict("workspace scope is not authorized"),
        LifecycleError::ScopeManagerUnavailable => AppError::service_unavailable(
            "no single live runner manager is observed for this workspace",
        ),
        LifecycleError::ScopeBusy => {
            AppError::conflict("workspace is busy with another execution owner")
        }
        LifecycleError::InvalidState => {
            AppError::conflict("execution environment is not in a state that admits this operation")
        }
        LifecycleError::Internal(error) => AppError::internal(error),
    }
}
