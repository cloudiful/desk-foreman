use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::upstream::native_task_preparation::support::Rig as PreparationRig;

pub(super) async fn report_job(
    fixture: &PreparationRig,
    job_id: &str,
    ok: bool,
    result: Option<Value>,
    error: Option<&str>,
) -> StatusCode {
    fixture
        .client
        .post(format!(
            "{}/api/internal/runner-manager/jobs/result",
            fixture.base
        ))
        .bearer_auth(&fixture.manager_token)
        .json(&json!({
            "job_id": job_id,
            "ok": ok,
            "result": result,
            "error": error,
        }))
        .send()
        .await
        .expect("report runner job result")
        .status()
}
