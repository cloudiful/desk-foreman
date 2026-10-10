//! Shared ack builders and helpers for the result-transport cases
//! (issue 742 P2).

use serde_json::{Value, json};
use uuid::Uuid;

use runner_protocol::RunnerOwner;

use super::http::http_get;

pub(super) const NEXT: &str = "/api/internal/runner-manager/jobs/next";
pub(super) const RESULT: &str = "/api/internal/runner-manager/jobs/result";

pub(super) fn owner(environment_id: Uuid, generation: i64) -> RunnerOwner {
    RunnerOwner::ExecutionEnvironment {
        environment_id: environment_id.to_string(),
        generation,
    }
}

pub(super) fn ensure_ack(environment_id: Uuid, generation: i64, state: &str) -> Value {
    let owner = owner(environment_id, generation);
    json!({
        "owner": owner,
        "container_name": owner.container_name(),
        "state": state,
    })
}

pub(super) fn stop_ack(
    environment_id: Uuid,
    generation: i64,
    confirmed: bool,
    observed_state: &str,
) -> Value {
    let owner = owner(environment_id, generation);
    json!({
        "owner": owner,
        "container_name": owner.container_name(),
        "confirmed": confirmed,
        "observed_state": observed_state,
    })
}

pub(super) fn result_body(
    job_id: &str,
    ok: bool,
    result: Option<Value>,
    error: Option<&str>,
) -> Value {
    json!({
        "job_id": job_id,
        "ok": ok,
        "result": result,
        "error": error,
    })
}

pub(super) async fn pull(base: &str, token: &str) -> String {
    let (status, body) = http_get(base, NEXT, Some(token)).await;
    assert_eq!(status, 200, "pull must succeed");
    body["job_id"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a pulled job, got {body}"))
        .to_string()
}
