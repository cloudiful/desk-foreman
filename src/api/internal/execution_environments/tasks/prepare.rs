//! Authenticated native-task preparation handlers (issue 753 P2b).
//!
//! Scheduling records a durable intent for the task's original manager to
//! materialize the task's sealed seed into a private checkout, and reading
//! returns that intent's current status. Both resolve the application identity
//! from the bearer first — never from the body.
//!
//! Nothing here dispatches to a manager, mounts anything, starts a container
//! or writes a canonical root: the job is durable control-plane state.

pub mod types;

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
        PrepareError, PrepareRequest, find_native_task_preparation, prepare_native_task,
    },
    error::AppError,
};

use types::{NativeTaskPreparationDescriptor, PrepareNativeTaskRequestApi};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    request_body = PrepareNativeTaskRequestApi,
    responses(
        (status = 200, body = NativeTaskPreparationDescriptor, description = "The scheduled preparation, or the existing one on an identical replay"),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse, description = "Environment, task or binding absent for this application"),
        (status = 409, body = crate::error::ErrorResponse, description = "Fence, task state, seed, manager, blocker or token conflict"),
        (status = 503, body = crate::error::ErrorResponse, description = "The task's original manager is unavailable")
    )
)]
pub async fn prepare_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    ValidatedJson(request): ValidatedJson<PrepareNativeTaskRequestApi>,
) -> Result<Json<NativeTaskPreparationDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = prepare_native_task(
        &state.db,
        application_id,
        environment_id,
        task_id,
        PrepareRequest {
            expected_generation: request.expected_generation,
            operation_token: request.operation_token,
        },
    )
    .await
    .map_err(map_prepare_error)?;
    Ok(Json(NativeTaskPreparationDescriptor::from(descriptor)))
}

#[utoipa::path(
    get,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    responses(
        (status = 200, body = NativeTaskPreparationDescriptor),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn find_native_task_preparation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<NativeTaskPreparationDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor =
        find_native_task_preparation(&state.db, application_id, environment_id, task_id)
            .await
            .map_err(map_prepare_error)?;
    Ok(Json(NativeTaskPreparationDescriptor::from(descriptor)))
}

/// Map a typed preparation refusal to a stable status.
///
/// No variant reveals whether a foreign task, environment, seed or manager
/// exists: a foreign or absent resource is a plain `404`.
fn map_prepare_error(error: PrepareError) -> AppError {
    match error {
        PrepareError::NotFound => AppError::not_found("native task preparation not found"),
        PrepareError::StaleGeneration => {
            AppError::conflict("expected generation does not match the stored generation")
        }
        PrepareError::TaskNotAdmitted => {
            AppError::conflict("task is not an admitted intent of this environment")
        }
        PrepareError::BindingIneligible => {
            AppError::conflict("workspace binding is not an active code project workspace")
        }
        PrepareError::SubjectMismatch => {
            AppError::conflict("task subject does not match the execution environment")
        }
        PrepareError::SeedMissing => {
            AppError::conflict("task has no sealed input snapshot to prepare")
        }
        PrepareError::EnvironmentNotIdle => {
            AppError::conflict("execution environment is not idle for a preparation")
        }
        PrepareError::ManagerUnavailable => {
            AppError::service_unavailable("the task's original manager is unavailable")
        }
        PrepareError::UnresolvedBlockers => {
            AppError::conflict("execution environment has unresolved lifecycle blockers")
        }
        PrepareError::TokenConflict => {
            AppError::conflict("operation token was already used with a different intent")
        }
        PrepareError::Internal => AppError::internal(anyhow::anyhow!(
            "native task preparation persistence failed"
        )),
    }
}
