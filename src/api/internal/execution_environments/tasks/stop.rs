use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use uuid::Uuid;

use crate::{
    AppState,
    api::{internal::shared::application_actor, validation::ValidatedJson},
    db::execution_environments::{TaskExecutionRequest, stop_native_task},
    error::AppError,
};

use super::{
    start::map_execution_error,
    types::{NativeTaskExecutionDescriptor, TaskExecutionRequestApi},
};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/stop",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable native task identifier")
    ),
    request_body = TaskExecutionRequestApi,
    responses(
        (status = 200, body = NativeTaskExecutionDescriptor, description = "The task stop operation; release remains a separate application call"),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn stop_native_task_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    ValidatedJson(request): ValidatedJson<TaskExecutionRequestApi>,
) -> Result<Json<NativeTaskExecutionDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = stop_native_task(
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
    if let Some(generation) = descriptor.relay_cancel_through_generation() {
        state
            .runner_broker
            .service_relay()
            .cancel_environment_through(environment_id, generation);
    }
    Ok(Json(descriptor.into()))
}
