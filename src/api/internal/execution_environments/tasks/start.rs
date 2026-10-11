use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use uuid::Uuid;

use crate::{
    AppState,
    api::{internal::shared::application_actor, validation::ValidatedJson},
    db::execution_environments::{TaskExecutionError, TaskExecutionRequest, start_native_task},
    error::AppError,
};

use super::types::{NativeTaskExecutionDescriptor, TaskExecutionRequestApi};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/start",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable native task identifier")
    ),
    request_body = TaskExecutionRequestApi,
    responses(
        (status = 200, body = NativeTaskExecutionDescriptor, description = "The task start operation or its current replay state"),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn start_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    ValidatedJson(request): ValidatedJson<TaskExecutionRequestApi>,
) -> Result<Json<NativeTaskExecutionDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = start_native_task(
        &state.db,
        application_id,
        environment_id,
        task_id,
        TaskExecutionRequest {
            expected_generation: request.expected_generation,
            operation_token: request.operation_token,
        },
    )
    .await
    .map_err(map_execution_error)?;
    Ok(Json(descriptor.into()))
}

pub(super) fn map_execution_error(error: TaskExecutionError) -> AppError {
    match error {
        TaskExecutionError::NotFound => AppError::not_found("native task not found"),
        TaskExecutionError::StaleGeneration => {
            AppError::conflict("expected generation does not match the stored generation")
        }
        TaskExecutionError::Conflict | TaskExecutionError::TokenConflict => {
            AppError::conflict("native task execution conflicts with its stored state")
        }
        TaskExecutionError::InvalidToken => {
            AppError::bad_request("operation token must be non-blank and bounded")
        }
        TaskExecutionError::ManagerUnavailable => {
            AppError::service_unavailable("the native task's original manager is unavailable")
        }
        TaskExecutionError::Internal => {
            AppError::internal(anyhow::anyhow!("native task execution persistence failed"))
        }
    }
}
