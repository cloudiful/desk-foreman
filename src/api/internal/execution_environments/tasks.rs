//! Authenticated durable native task handlers (issue 764 P1).
//!
//! Routes admit and inspect one task, seal and prepare its seed, schedule
//! original-manager start/stop jobs, and release only against a positive stop
//! proof. Every application route resolves identity from the bearer and uses
//! owner-scoped state. This HTTP layer never accepts host paths or starts
//! Docker; running and stop-proof states require matching manager acknowledgments.

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
        NativeTaskError, NativeTaskRequest, admit_native_task, cancel_native_task,
        find_native_task, release_native_task,
    },
    error::AppError,
};

pub mod prepare;
pub mod seeds;
pub mod start;
pub mod stop;
pub mod types;

pub use prepare::{find_native_task_preparation_handler, prepare_native_task_handler};
pub use seeds::{find_native_task_seed_handler, seed_native_task_handler};
pub use start::start_native_task_handler;
pub use stop::stop_native_task_handler;

use types::{
    AdmitNativeTaskRequest, CancelNativeTaskRequest, NativeTaskDescriptor, ReleaseNativeTaskRequest,
};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/admit",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier")
    ),
    request_body = AdmitNativeTaskRequest,
    responses(
        (status = 200, body = NativeTaskDescriptor, description = "The admitted task, or the existing task on an identical replay"),
        (status = 400, body = crate::error::ErrorResponse, description = "Operation token is missing or oversized"),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse, description = "Environment or binding absent for this application"),
        (status = 409, body = crate::error::ErrorResponse, description = "Fence, subject, binding eligibility, legacy lease owner, unresolved blockers or an active task conflict"),
        (status = 503, body = crate::error::ErrorResponse, description = "No runner manager is available to bind")
    )
)]
pub async fn admit_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(environment_id): Path<Uuid>,
    ValidatedJson(request): ValidatedJson<AdmitNativeTaskRequest>,
) -> Result<Json<NativeTaskDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let task = admit_native_task(
        &state.db,
        application_id,
        environment_id,
        NativeTaskRequest {
            task_id: request.task_id,
            workspace_binding_id: request.workspace_binding_id,
            subject_external_user_id: request.subject_external_user_id,
            expected_generation: request.expected_generation,
            operation_token: request.operation_token,
        },
    )
    .await
    .map_err(map_task_error)?;
    Ok(Json(NativeTaskDescriptor::from_task(&task)))
}

#[utoipa::path(
    get,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    responses(
        (status = 200, body = NativeTaskDescriptor),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn find_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<NativeTaskDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let task = find_native_task(&state.db, application_id, environment_id, task_id)
        .await
        .map_err(map_task_error)?;
    Ok(Json(NativeTaskDescriptor::from_task(&task)))
}

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/cancel",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    request_body = CancelNativeTaskRequest,
    responses(
        (status = 200, body = NativeTaskDescriptor, description = "The voided task, or the uncertain task when something may have been dispatched"),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse, description = "The caller's generation fence does not match")
    )
)]
pub async fn cancel_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    ValidatedJson(request): ValidatedJson<CancelNativeTaskRequest>,
) -> Result<Json<NativeTaskDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let task = cancel_native_task(
        &state.db,
        application_id,
        environment_id,
        task_id,
        request.expected_generation,
    )
    .await
    .map_err(map_task_error)?;
    Ok(Json(NativeTaskDescriptor::from_task(&task)))
}

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/release",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    request_body = ReleaseNativeTaskRequest,
    responses(
        (status = 200, body = NativeTaskDescriptor, description = "The released task; a repeated release is idempotent"),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse, description = "Fence mismatch, or the named proof does not match this task's manager, generation, link and confirmation")
    )
)]
pub async fn release_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    ValidatedJson(request): ValidatedJson<ReleaseNativeTaskRequest>,
) -> Result<Json<NativeTaskDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let task = release_native_task(
        &state.db,
        application_id,
        environment_id,
        task_id,
        request.expected_generation,
        request.expected_container_generation,
        request.expected_stop_job_id,
    )
    .await
    .map_err(map_task_error)?;
    Ok(Json(NativeTaskDescriptor::from_task(&task)))
}

/// Map a typed task refusal to a stable HTTP status.
///
/// No variant exposes a task, binding, proof, manager detail, host path or
/// credential. Existence is never confirmed across an ownership boundary: a
/// foreign task, binding or proof is simply absent.
pub(super) fn map_task_error(error: NativeTaskError) -> AppError {
    match error {
        NativeTaskError::NotFound => AppError::not_found("native task not found"),
        NativeTaskError::StaleGeneration => {
            AppError::conflict("expected generation does not match the stored generation")
        }
        NativeTaskError::SubjectMismatch => {
            AppError::conflict("subject does not match the execution environment")
        }
        NativeTaskError::BindingIneligible => {
            AppError::conflict("workspace binding is not an active code project workspace")
        }
        NativeTaskError::BindingLifecycle => {
            AppError::conflict("workspace binding lifecycle does not allow a native task")
        }
        NativeTaskError::LegacyOwnerPresent => {
            AppError::conflict("workspace binding still carries a legacy write lease owner")
        }
        NativeTaskError::TaskAlreadyActive => {
            AppError::conflict("an active native task already holds this environment or workspace")
        }
        NativeTaskError::TokenConflict => {
            AppError::conflict("operation token was already used with a different intent")
        }
        NativeTaskError::UnresolvedBlockers => {
            AppError::conflict("execution environment has unresolved lifecycle blockers")
        }
        NativeTaskError::EnvironmentNotIdle => {
            AppError::conflict("execution environment is not in an idle state for a native task")
        }
        NativeTaskError::NoManagerAvailable | NativeTaskError::ManagerUnavailable => {
            AppError::service_unavailable("no runner manager is available to bind the task")
        }
        NativeTaskError::AlreadyReleased => {
            AppError::conflict("native task has already been released")
        }
        NativeTaskError::ProofMismatch => {
            AppError::conflict("stop proof does not match this task's durable evidence")
        }
        NativeTaskError::InvalidToken => {
            AppError::bad_request("operation token must be non-blank and bounded")
        }
        NativeTaskError::Internal => {
            AppError::internal(anyhow::anyhow!("native task persistence failed"))
        }
    }
}
