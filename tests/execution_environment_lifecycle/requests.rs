//! Lifecycle request helpers for the integration suite (issue 742 P3).

use serde_json::{Value, json};
use uuid::Uuid;

use super::http::{http_get, http_post};

/// Wrap a raw descriptor job UUID into the reserved manager wire id.
pub fn wire_job_id(job_id: &str) -> String {
    format!("envjob:{job_id}")
}

/// Admit a provision and return `(status, body)`.
pub async fn provision(
    base: &str,
    token: &str,
    environment_id: Uuid,
    expected_generation: i64,
    operation_token: &str,
) -> (u16, Value) {
    http_post(
        base,
        &format!("/api/internal/execution-environments/{environment_id}/provision"),
        Some(token),
        json!({"expected_generation": expected_generation, "operation_token": operation_token}),
    )
    .await
}

/// Admit a stop and return `(status, body)`.
pub async fn stop(
    base: &str,
    token: &str,
    environment_id: Uuid,
    expected_generation: i64,
    operation_token: &str,
) -> (u16, Value) {
    http_post(
        base,
        &format!("/api/internal/execution-environments/{environment_id}/stop"),
        Some(token),
        json!({"expected_generation": expected_generation, "operation_token": operation_token}),
    )
    .await
}

/// Admit a replace and return `(status, body)`.
pub async fn replace(
    base: &str,
    token: &str,
    environment_id: Uuid,
    expected_generation: i64,
    expected_container_generation: i64,
    expected_stop_job_id: &str,
    operation_token: &str,
) -> (u16, Value) {
    http_post(
        base,
        &format!("/api/internal/execution-environments/{environment_id}/replace"),
        Some(token),
        json!({
            "expected_generation": expected_generation,
            "expected_container_generation": expected_container_generation,
            "expected_stop_job_id": expected_stop_job_id,
            "operation_token": operation_token,
        }),
    )
    .await
}

/// Read the lifecycle status and return `(status, body)`.
pub async fn status(base: &str, token: &str, environment_id: Uuid) -> (u16, Value) {
    http_get(
        base,
        &format!("/api/internal/execution-environments/{environment_id}/status"),
        Some(token),
    )
    .await
}
