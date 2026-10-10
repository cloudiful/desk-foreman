//! Owner-scoped private-service gateway endpoints (issue 749 P2b).
//!
//! One implementation, seven declared operations. The methods are separate
//! functions because the generated document must describe each one
//! individually; they all refuse the same things and relay through the same
//! path, so there is exactly one behaviour to reason about.
//!
//! The caller never names a target. There is no user-supplied manager id, no
//! user-supplied host, and no user-supplied port anywhere on this path: the
//! environment's immutable manager binding is the only destination, and the
//! agent's own address is chosen on the manager side.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Method, Uri},
    response::Response,
};
use uuid::Uuid;

use crate::{AppState, api::internal::shared::application_actor, error::AppError};

use super::{
    BoundsRefusal, SERVICE_GENERATION_HEADER, bounds::bounded_service_request,
    relay::relay_service_request,
};

/// Declare one relayed method with its own documented operation.
///
/// `utoipa` registers a single method per `path` attribute, so a shared
/// implementation with seven entry points is what keeps the generated
/// document faithful instead of silently documenting only the last method.
macro_rules! relayed_method {
    ($handler:ident, $verb:ident, $upper:ident) => {
        #[utoipa::path(
            $verb,
            path = "/api/internal/execution-environments/{environment_id}/service/{path}",
            tag = "internal",
            security(("application_bearer" = [])),
            params(
                ("environment_id" = Uuid, Path, description = "Durable environment identifier"),
                ("path" = String, Path, description = "Bounded relative path inside the private agent service")
            ),
            responses(
                (status = 200, description = "Relayed agent status and streamed body; server-sent events and binary bodies are relayed incrementally"),
                (status = 400, body = crate::error::ErrorResponse, description = "Unsupported method, unparseable path/query/header, or missing Content-Length"),
                (status = 401, body = crate::error::ErrorResponse),
                (status = 404, body = crate::error::ErrorResponse, description = "Foreign or unknown environment, or an agent resource that does not exist"),
                (status = 409, body = crate::error::ErrorResponse, description = "Fence mismatch, environment not ready, or the relay cannot admit the stream"),
                (status = 413, body = crate::error::ErrorResponse, description = "Declared body length over the relay bound"),
                (status = 503, body = crate::error::ErrorResponse, description = "No live manager tunnel, or the tunnel ended the stream"),
                (status = 504, body = crate::error::ErrorResponse, description = "The agent service timed out")
            )
        )]
        pub async fn $handler(
            State(state): State<AppState>,
            headers: HeaderMap,
            Path((environment_id, raw_path)): Path<(Uuid, String)>,
            uri: Uri,
            body: Body,
        ) -> Result<Response, AppError> {
            serve_service_request(state, headers, environment_id, raw_path, Method::$upper, uri, body)
                .await
        }
    };
}

relayed_method!(execution_environment_service_get, get, GET);
relayed_method!(execution_environment_service_post, post, POST);
relayed_method!(execution_environment_service_put, put, PUT);
relayed_method!(execution_environment_service_patch, patch, PATCH);
relayed_method!(execution_environment_service_delete, delete, DELETE);
relayed_method!(execution_environment_service_head, head, HEAD);
relayed_method!(execution_environment_service_options, options, OPTIONS);

/// Bind every relayed method onto one route.
pub fn service_route() -> axum::routing::MethodRouter<AppState> {
    axum::routing::get(execution_environment_service_get)
        .post(execution_environment_service_post)
        .put(execution_environment_service_put)
        .patch(execution_environment_service_patch)
        .delete(execution_environment_service_delete)
        .head(execution_environment_service_head)
        .options(execution_environment_service_options)
}

/// The one behaviour every relayed method shares.
async fn serve_service_request(
    state: AppState,
    headers: HeaderMap,
    environment_id: Uuid,
    raw_path: String,
    method: Method,
    uri: Uri,
    body: Body,
) -> Result<Response, AppError> {
    let application_id = application_actor(&state, &headers).await?;
    let presented_generation = presented_generation(&headers)?;
    let bounds = match bounded_service_request(&method, &raw_path, uri.query(), &headers) {
        Ok(bounds) => bounds,
        Err(BoundsRefusal::BodyTooLarge) => return Ok(body_too_large_response()),
        Err(refusal) => return Err(refuse_bounds(refusal)),
    };
    relay_service_request(
        state.runner_broker.service_relay(),
        &state.db,
        application_id,
        environment_id,
        presented_generation,
        method.as_str(),
        bounds,
        body,
    )
    .await
}

/// Read the caller's expected generation fence.
///
/// Required on every service request: without it the relay would have no
/// generation to fence the stream against, and admitting one anyway would
/// reopen the invalidate race this design exists to close.
fn presented_generation(headers: &HeaderMap) -> Result<i64, AppError> {
    let raw = headers
        .get(SERVICE_GENERATION_HEADER)
        .ok_or_else(|| {
            AppError::bad_request(format!("{SERVICE_GENERATION_HEADER} header is required"))
        })?
        .to_str()
        .map_err(|_| AppError::bad_request("generation fence header is malformed"))?;
    raw.parse()
        .map_err(|_| AppError::bad_request("generation fence header is malformed"))
}

/// Map a bounds refusal to its documented status.
///
/// The message names the rule, never the offending value, so a refusal
/// cannot be used to probe the relay's internals.
///
/// An over-bound body is a size refusal, not a malformed request: the
/// published contract answers 413 for a well-formed length above the relay
/// bound while a missing or unparseable `Content-Length` stays 400. The
/// envelope is built here rather than through a new `AppError` variant so
/// the response body stays identical to every other error on this path.
fn refuse_bounds(refusal: BoundsRefusal) -> AppError {
    match refusal {
        BoundsRefusal::UnsupportedMethod => AppError::bad_request("method is not relayable"),
        BoundsRefusal::InvalidPath => {
            AppError::bad_request("service path must be a bounded relative reference")
        }
        BoundsRefusal::InvalidQuery => AppError::bad_request("service query is not bounded"),
        BoundsRefusal::InvalidHeader => {
            AppError::bad_request("a forwarded header is oversized or malformed")
        }
        BoundsRefusal::InvalidBodyLength => {
            AppError::bad_request("content-length is required and must be a number")
        }
        BoundsRefusal::BodyTooLarge => AppError::bad_request(BODY_TOO_LARGE_MESSAGE),
    }
}

/// Message carried by the 413 body-size refusal.
const BODY_TOO_LARGE_MESSAGE: &str = "declared body length exceeds the relay bound";

/// Render the 413 size refusal.
///
/// Returned directly because the shared error type carries no size variant
/// and this phase may not widen it; the shape is exactly what every other
/// error on this path produces.
fn body_too_large_response() -> Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        axum::Json(crate::error::ErrorResponse {
            error: BODY_TOO_LARGE_MESSAGE.to_string(),
        }),
    )
        .into_response()
}
