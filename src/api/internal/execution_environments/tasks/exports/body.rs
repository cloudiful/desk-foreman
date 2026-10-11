use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{StatusCode, header::CONTENT_LENGTH},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;

use crate::error::ErrorResponse;

pub(crate) async fn decode_task_export_body<T: DeserializeOwned>(
    request: Request,
) -> Result<T, Response> {
    let limit = runner_protocol::MAX_TASK_EXPORT_UPLOAD_BODY_BYTES;
    if request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
    {
        return Err(too_large());
    }
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| too_large())?;
    serde_json::from_slice(&bytes).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid task export request body".to_string(),
            }),
        )
            .into_response()
    })
}

fn too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(ErrorResponse {
            error: "task export request body exceeds its limit".to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::*;

    #[tokio::test]
    async fn actual_body_limit_does_not_trust_a_smaller_declared_length() {
        let request = Request::builder()
            .header(CONTENT_LENGTH, "1")
            .body(Body::from(vec![
                b'x';
                runner_protocol::MAX_TASK_EXPORT_UPLOAD_BODY_BYTES
                    + 1
            ]))
            .unwrap();
        let response = decode_task_export_body::<serde_json::Value>(request)
            .await
            .unwrap_err();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn oversized_declared_length_is_rejected_before_empty_body_decoding() {
        let request = Request::builder()
            .header(
                CONTENT_LENGTH,
                (runner_protocol::MAX_TASK_EXPORT_UPLOAD_BODY_BYTES + 1).to_string(),
            )
            .body(Body::empty())
            .unwrap();
        let response = decode_task_export_body::<serde_json::Value>(request)
            .await
            .unwrap_err();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
