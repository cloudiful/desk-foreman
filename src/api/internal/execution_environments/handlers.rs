//! Authenticated handlers for durable execution environments (issue 737 P1).
//!
//! Both routes reuse `application_actor` so the application identity is
//! bearer-derived and never request-supplied. `ensure` is idempotent via
//! a unique-index-safe upsert; `invalidate` is an owner-scoped atomic
//! conditional update. No Docker or provisioning call is reachable here,
//! and invalidation never claims to stop a writer: this phase has no
//! writers and no routing.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use uuid::Uuid;

use crate::{
    AppState,
    api::{internal::shared::application_actor, validation::ValidatedJson},
    db::{
        execution_environments::{EnsureOutcome, InvalidateOutcome},
        types::ExecutionEnvironment,
    },
    error::AppError,
};

use super::types::{EnsureExecutionEnvironmentRequest, InvalidateExecutionEnvironmentRequest};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/ensure",
    tag = "internal",
    security(("application_bearer" = [])),
    request_body = EnsureExecutionEnvironmentRequest,
    responses(
        (status = 200, body = ExecutionEnvironment),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn ensure_execution_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<EnsureExecutionEnvironmentRequest>,
) -> Result<Json<ExecutionEnvironment>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    match crate::db::queries::ensure_execution_environment(
        &state.db,
        application_id,
        &request.external_user_id,
        &request.runtime_kind,
    )
    .await?
    {
        EnsureOutcome::Created(environment) | EnsureOutcome::Existing(environment) => {
            Ok(Json(environment))
        }
        EnsureOutcome::RuntimeConflict { current } => Err(AppError::conflict(format!(
            "runtime kind conflict: environment {} uses '{}'",
            current.environment_id, current.runtime_kind
        ))),
    }
}

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/invalidate",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier")
    ),
    request_body = InvalidateExecutionEnvironmentRequest,
    responses(
        (status = 200, body = ExecutionEnvironment),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn invalidate_execution_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(environment_id): Path<Uuid>,
    ValidatedJson(request): ValidatedJson<InvalidateExecutionEnvironmentRequest>,
) -> Result<Json<ExecutionEnvironment>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    match crate::db::queries::invalidate_execution_environment(
        &state.db,
        application_id,
        environment_id,
        request.expected_generation,
    )
    .await?
    {
        // Only a committed invalidation may tear live streams down. A stale
        // or missing environment changed nothing, so its streams keep running
        // against the generation that is still authoritative.
        InvalidateOutcome::Invalidated(environment) => {
            state
                .runner_broker
                .service_relay()
                .cancel_environment_through(environment_id, request.expected_generation);
            Ok(Json(environment))
        }
        InvalidateOutcome::Stale { current } => Err(AppError::conflict(format!(
            "stale generation: expected {} but environment {} is at generation {}",
            request.expected_generation, current.environment_id, current.generation
        ))),
        InvalidateOutcome::NativeTaskActive => Err(AppError::conflict(
            "execution environment is owned by an active native task",
        )),
        InvalidateOutcome::NotFound => Err(AppError::not_found("execution environment not found")),
    }
}
