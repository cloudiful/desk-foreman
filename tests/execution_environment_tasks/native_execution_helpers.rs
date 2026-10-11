use runner_protocol::RunnerOwner;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{TaskFixture, call};

pub async fn prepare_task(fixture: &TaskFixture, task_id: Uuid) {
    let (job_id, job) = schedule_preparation_job(fixture, task_id).await;
    let payload = &job["payload"];
    assert_eq!(
        fixture
            .manager_result(
                job_id,
                true,
                json!({
                    "task_id": task_id,
                    "manifest_sha256": payload["manifest_sha256"],
                    "file_count": payload["file_count"],
                    "total_bytes": payload["total_bytes"],
                    "prepared": true
                }),
            )
            .await
            .status,
        204
    );
}

pub async fn schedule_preparation_job(fixture: &TaskFixture, task_id: Uuid) -> (Uuid, Value) {
    assert_eq!(
        fixture.admit_default(task_id, "task-admit").await.status,
        200
    );
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &[("README.md".to_string(), "seed".to_string())],
            )
            .await
            .status,
        200
    );
    let preparation = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            json!({"expected_generation": 4, "operation_token": "task-prepare"}),
        )
        .await;
    assert_eq!(preparation.status, 200, "{}", preparation.body_text());
    let (job_id, job) = poll_task_job(fixture).await;
    assert_eq!(job["kind"], "prepare_native_task");
    (job_id, job)
}

pub async fn poll_task_job(fixture: &TaskFixture) -> (Uuid, Value) {
    let response = call(
        &fixture.base,
        "GET",
        "/api/internal/runner-manager/jobs/next",
        Some(&fixture.manager_token),
        &[],
        b"",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_text());
    let job: Value = serde_json::from_slice(&response.body).expect("job JSON");
    assert!(!job.is_null(), "expected a durable task job");
    (parse_job_id(&job), job)
}

pub fn parse_job_id(job: &Value) -> Uuid {
    Uuid::parse_str(
        job["job_id"]
            .as_str()
            .expect("wire job id")
            .strip_prefix("envjob:")
            .expect("reserved durable job id"),
    )
    .expect("job UUID")
}

pub fn start_result(job: &Value) -> Value {
    let owner: RunnerOwner =
        serde_json::from_value(job["payload"]["owner"].clone()).expect("native task owner");
    json!({
        "task": job["payload"]["task"],
        "environment": {
            "owner": owner,
            "container_name": owner.container_name(),
            "state": "running"
        }
    })
}

pub fn stop_result(job: &Value, preparation_quiescent: bool) -> Value {
    let owner: RunnerOwner =
        serde_json::from_value(job["payload"]["owner"].clone()).expect("native task owner");
    json!({
        "task": job["payload"]["task"],
        "environment": {
            "owner": owner,
            "container_name": owner.container_name(),
            "confirmed": true,
            "observed_state": "exited"
        },
        "preparation_quiescent": preparation_quiescent
    })
}
