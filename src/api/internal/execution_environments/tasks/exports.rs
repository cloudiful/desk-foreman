pub(crate) mod body;
pub mod types;

use axum::{
    Json,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use uuid::Uuid;
use validator::Validate;

use crate::{
    AppState,
    api::{internal::shared::application_actor, validation::validation_errors_message},
    db::execution_environments::tasks::exports::{
        NativeTaskExportError, find_native_task_export, find_native_task_export_content,
        schedule_native_task_export,
    },
    error::{AppError, ErrorResponse},
};

use types::{NativeTaskExportDescriptor, ScheduleNativeTaskExportRequest};

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/export",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable native task identifier")
    ),
    request_body = ScheduleNativeTaskExportRequest,
    responses(
        (status = 200, body = NativeTaskExportDescriptor, description = "The immutable selected-file export intent, or its exact replay"),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 413, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn schedule_native_task_export_handler(
    State(state): State<AppState>,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let application_id = match application_actor(&state, &headers).await {
        Ok(application_id) => application_id,
        Err(error) => return error.into_response(),
    };
    let parsed: ScheduleNativeTaskExportRequest = match body::decode_task_export_body(request).await
    {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    if let Err(errors) = parsed.validate() {
        return bad_request(&validation_errors_message(&errors));
    }
    let selection = match runner_protocol::validate_native_task_export_selection(&parsed.paths) {
        Ok(selection) => selection,
        Err(error) if error.is_too_large() => {
            return payload_too_large("export selection exceeds the accepted limits");
        }
        Err(error) => return bad_request(error.code()),
    };
    match schedule_native_task_export(
        &state.db,
        application_id,
        environment_id,
        task_id,
        &parsed.operation_token,
        parsed.stop_job_id,
        &selection.paths,
    )
    .await
    {
        Ok(descriptor) => Json(NativeTaskExportDescriptor::from(descriptor)).into_response(),
        Err(error) => map_export_error(error).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/export/status",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable native task identifier")
    ),
    responses(
        (status = 200, body = NativeTaskExportDescriptor),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn find_native_task_export_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<NativeTaskExportDescriptor>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let descriptor = find_native_task_export(&state.db, application_id, environment_id, task_id)
        .await
        .map_err(map_export_error)?;
    Ok(Json(NativeTaskExportDescriptor::from(descriptor)))
}

#[utoipa::path(
    get,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/export/content",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable native task identifier")
    ),
    responses(
        (status = 200, body = runner_protocol::NativeTaskExportContent),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse, description = "Export output is not ready")
    )
)]
pub async fn find_native_task_export_content_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<runner_protocol::NativeTaskExportContent>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let content =
        find_native_task_export_content(&state.db, application_id, environment_id, task_id)
            .await
            .map_err(map_export_error)?;
    Ok(Json(content))
}

pub(super) fn map_export_error(error: NativeTaskExportError) -> AppError {
    match error {
        NativeTaskExportError::NotFound => AppError::not_found("native task export not found"),
        NativeTaskExportError::InvalidToken => {
            AppError::bad_request("operation token must be non-blank and bounded")
        }
        NativeTaskExportError::InvalidSelection(error)
        | NativeTaskExportError::InvalidContent(error) => {
            if error.is_too_large() {
                AppError::bad_request(error.code())
            } else if error.is_conflict() {
                AppError::conflict(error.code())
            } else {
                AppError::bad_request(error.code())
            }
        }
        NativeTaskExportError::Conflict => {
            AppError::conflict("export intent or output does not match the stored task")
        }
        NativeTaskExportError::ManagerUnavailable => {
            AppError::service_unavailable("the task's original runner manager is unavailable")
        }
        NativeTaskExportError::NotReady | NativeTaskExportError::NotCompletable => {
            AppError::conflict("native task export output is not ready")
        }
        NativeTaskExportError::Internal => {
            AppError::internal(anyhow::anyhow!("native task export persistence failed"))
        }
    }
}

fn payload_too_large(message: &str) -> Response {
    error_response(StatusCode::PAYLOAD_TOO_LARGE, message)
}

fn bad_request(message: &str) -> Response {
    error_response(StatusCode::BAD_REQUEST, message)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}
