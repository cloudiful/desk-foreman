use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::{
    AppState,
    db::execution_environments::tasks::exports::{
        NativeTaskExportError, NativeTaskExportUploadOutcome, upload_native_task_export,
    },
    error::{AppError, ErrorResponse},
};

#[utoipa::path(
    post,
    path = "/api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-export",
    tag = "internal",
    params(
        ("manager_id" = i64, Path, description = "Authenticated runner manager identifier"),
        ("job_id" = Uuid, Path, description = "Claimed native task export job identifier")
    ),
    request_body = runner_protocol::ExportNativeTaskUpload,
    responses(
        (status = 204, description = "Immutable selected output committed"),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse, description = "Foreign, absent or non-export job"),
        (status = 409, body = ErrorResponse, description = "Export claim or immutable output conflicts"),
        (status = 413, body = ErrorResponse, description = "Upload body or selected output exceeds its bound")
    )
)]
pub async fn upload_native_task_export_handler(
    State(state): State<AppState>,
    Path((manager_id, job_id)): Path<(i64, Uuid)>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let manager =
        match super::runner_manager::runner_manager_from_token(&state, &parts.headers).await {
            Ok(manager) => manager,
            Err(error) => return error.into_response(),
        };
    if manager.runner_manager_id != manager_id {
        return error_response(StatusCode::NOT_FOUND, "task export job not found");
    }
    let upload =
        match super::execution_environments::tasks::exports::body::decode_task_export_body::<
            runner_protocol::ExportNativeTaskUpload,
        >(Request::from_parts(parts, body))
        .await
        {
            Ok(upload) => upload,
            Err(response) => return response,
        };
    match upload_native_task_export(&state.db, manager_id, job_id, upload).await {
        Ok(
            NativeTaskExportUploadOutcome::Uploaded
            | NativeTaskExportUploadOutcome::AlreadyUploaded,
        ) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_export_error(error),
    }
}

fn map_export_error(error: NativeTaskExportError) -> Response {
    match error {
        NativeTaskExportError::InvalidSelection(error)
        | NativeTaskExportError::InvalidContent(error) => {
            if error.is_too_large() {
                payload_too_large()
            } else if error.is_conflict() {
                error_response(StatusCode::CONFLICT, error.code())
            } else {
                error_response(StatusCode::BAD_REQUEST, error.code())
            }
        }
        NativeTaskExportError::NotFound => {
            error_response(StatusCode::NOT_FOUND, "task export job not found")
        }
        NativeTaskExportError::InvalidToken => error_response(
            StatusCode::BAD_REQUEST,
            "invalid task export operation token",
        ),
        NativeTaskExportError::Conflict => error_response(
            StatusCode::CONFLICT,
            "task export output conflicts with its intent",
        ),
        NativeTaskExportError::ManagerUnavailable => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "original runner manager is unavailable",
        ),
        NativeTaskExportError::NotReady | NativeTaskExportError::NotCompletable => {
            error_response(StatusCode::CONFLICT, "task export job is not claimable")
        }
        NativeTaskExportError::Internal => {
            AppError::internal(anyhow::anyhow!("native task export persistence failed"))
                .into_response()
        }
    }
}

fn payload_too_large() -> Response {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "task export upload body or selected output exceeds its limit",
    )
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}
