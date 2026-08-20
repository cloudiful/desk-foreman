use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::db::types::WorkspaceLeaseTakeoverConflict;

#[derive(Debug)]
pub enum AppError {
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    Conflict(String),
    BadRequest(String),
    ServiceUnavailable(String),
    /// Structured 409 conflict with a typed body for the lease takeover
    /// endpoint. Carries enough state for callers (e.g. stock) to determine
    /// the current lease owner and last refresh time without parsing
    /// human-readable strings.
    TakeoverConflict(WorkspaceLeaseTakeoverConflict),
    Internal(anyhow::Error),
}

#[derive(serde::Deserialize, Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

impl AppError {
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized(message.into())
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::ServiceUnavailable(message.into())
    }

    pub fn internal(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized(message) => error_response(StatusCode::UNAUTHORIZED, message),
            Self::Forbidden(message) => error_response(StatusCode::FORBIDDEN, message),
            Self::NotFound(message) => error_response(StatusCode::NOT_FOUND, message),
            Self::Conflict(message) => error_response(StatusCode::CONFLICT, message),
            Self::BadRequest(message) => error_response(StatusCode::BAD_REQUEST, message),
            Self::ServiceUnavailable(message) => {
                error_response(StatusCode::SERVICE_UNAVAILABLE, message)
            }
            Self::TakeoverConflict(conflict) => {
                (StatusCode::CONFLICT, Json(conflict)).into_response()
            }
            Self::Internal(error) => {
                tracing::error!(error = %error, "request failed");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        }
    }
}

fn error_response(status: StatusCode, message: String) -> Response {
    (status, Json(ErrorResponse { error: message })).into_response()
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self::Internal(value.into())
    }
}

#[cfg(test)]
mod tests {
    use axum::{http::StatusCode, response::IntoResponse};

    use super::AppError;

    #[test]
    fn runner_unavailable_errors_return_service_unavailable() {
        let response =
            AppError::service_unavailable("workspace runner is unavailable").into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
