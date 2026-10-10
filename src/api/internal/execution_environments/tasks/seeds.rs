//! Authenticated immutable task input seed handlers (issue 753 P2a).
//!
//! Two operations on one task's sealed snapshot: seal it, and read it back.
//! Both resolve the application identity from the bearer first — never from
//! the body — and neither exposes a host path, a canonical root, a manager or
//! a credential.
//!
//! The seal request body is bounded by the transport before it is decoded, so
//! an oversized payload is refused without parsing it. Seed contents are an
//! authorized artifact: they cross this boundary by design and are never
//! written to a log, an error message or a response header.

pub mod types;

use axum::{
    Json,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header::CONTENT_LENGTH},
    response::{IntoResponse, Response},
};
use uuid::Uuid;
use validator::Validate;

use crate::{
    AppState,
    api::{internal::shared::application_actor, validation::validation_errors_message},
    db::execution_environments::tasks::seeds::{
        SeedBoundsError, SeedError, SeedFile, find_native_task_seed, plan_task_seed,
        seal_native_task_seed,
    },
    error::{AppError, ErrorResponse},
};

use types::{NativeTaskSeedContent, NativeTaskSeedDescriptor, SeedNativeTaskRequest};

/// Transport cap for one seed request body, applied before JSON decoding.
///
/// This is larger than the aggregate content ceiling because JSON escaping can
/// expand contents; it bounds the request regardless.
const MAX_SEED_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;

#[utoipa::path(
    post,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    request_body = SeedNativeTaskRequest,
    responses(
        (status = 200, body = NativeTaskSeedDescriptor, description = "The sealed snapshot, or the existing identical snapshot on an exact replay"),
        (status = 400, body = crate::error::ErrorResponse, description = "Malformed request or an invalid path"),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse, description = "Environment, task or binding absent for this application"),
        (status = 409, body = crate::error::ErrorResponse, description = "Fence, task state, candidate, job reference or conflicting content"),
        (status = 413, body = crate::error::ErrorResponse, description = "Request body, entry count or content size over the accepted limit")
    )
)]
pub async fn seed_native_task_handler(
    State(state): State<AppState>,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let application_id = match application_actor(&state, &headers).await {
        Ok(application_id) => application_id,
        Err(error) => return error.into_response(),
    };

    // A declared length over the cap is refused without reading the body.
    if let Some(length) = content_length(&headers)
        && length > MAX_SEED_REQUEST_BODY_BYTES as u64
    {
        return payload_too_large();
    }
    let bytes = match to_bytes(request.into_body(), MAX_SEED_REQUEST_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return payload_too_large(),
    };

    let parsed: SeedNativeTaskRequest = match serde_json::from_slice(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid seed request body");
        }
    };
    if let Err(errors) = parsed.validate() {
        return error_response(StatusCode::BAD_REQUEST, &validation_errors_message(&errors));
    }

    let files: Vec<SeedFile> = parsed
        .files
        .into_iter()
        .map(|file| SeedFile {
            path: file.path,
            content: file.content,
        })
        .collect();
    let plan = match plan_task_seed(&files) {
        Ok(plan) => plan,
        Err(error) => return error_response(bounds_status(error), error.code()),
    };

    match seal_native_task_seed(
        &state.db,
        application_id,
        environment_id,
        task_id,
        parsed.expected_generation,
        &plan,
    )
    .await
    {
        Ok(descriptor) => Json(NativeTaskSeedDescriptor::from(descriptor)).into_response(),
        Err(error) => map_seed_error(error).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
        ("task_id" = Uuid, Path, description = "Stable task owner identifier")
    ),
    responses(
        (status = 200, body = NativeTaskSeedContent, description = "The sealed snapshot with its contents, returned unchanged in any task state"),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse, description = "Environment, task or snapshot absent for this application")
    )
)]
pub async fn find_native_task_seed_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((environment_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<NativeTaskSeedContent>, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let content = find_native_task_seed(&state.db, application_id, environment_id, task_id)
        .await
        .map_err(map_seed_error)?;
    Ok(Json(NativeTaskSeedContent::from(content)))
}

/// The declared request body length, when the caller supplied one.
fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

/// The status a content-rule refusal maps to.
///
/// Size ceilings are `413`; every shape rule is `400`. The split is decided by
/// the bounds type, not by the HTTP layer.
fn bounds_status(error: SeedBoundsError) -> StatusCode {
    match error {
        SeedBoundsError::TooManyEntries
        | SeedBoundsError::ContentTooLarge
        | SeedBoundsError::TotalTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        SeedBoundsError::InvalidPath
        | SeedBoundsError::PathTooLong
        | SeedBoundsError::DuplicatePath
        | SeedBoundsError::AncestorCollision
        | SeedBoundsError::ContentNotSupported => StatusCode::BAD_REQUEST,
    }
}

/// Map a stored-state refusal to a stable status.
///
/// No variant reveals whether a foreign task, environment or snapshot exists:
/// a foreign or absent resource is a plain `404`.
fn map_seed_error(error: SeedError) -> AppError {
    match error {
        SeedError::NotFound => AppError::not_found("native task input snapshot not found"),
        SeedError::StaleGeneration => {
            AppError::conflict("expected generation does not match the stored generation")
        }
        SeedError::TaskNotAdmitted => {
            AppError::conflict("task is not an admitted intent of this environment")
        }
        SeedError::CandidatePresent => {
            AppError::conflict("task already has a candidate container generation")
        }
        SeedError::JobReferencePresent => {
            AppError::conflict("a durable job already references this task")
        }
        SeedError::BindingIneligible => {
            AppError::conflict("workspace binding is not an active code project workspace")
        }
        SeedError::DifferentContent => {
            AppError::conflict("a different snapshot is already sealed for this task")
        }
        SeedError::Internal => {
            AppError::internal(anyhow::anyhow!("native task seed persistence failed"))
        }
    }
}

/// A bounded plain error response, for the statuses `AppError` does not carry.
fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}

/// The over-limit refusal shared by both body checks.
fn payload_too_large() -> Response {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "seed request body or content exceeds the accepted limit",
    )
}
