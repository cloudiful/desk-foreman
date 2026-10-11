use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::upstream_service_relay::full_gateway_support::support::NoopRunner;
use reqwest::{Response, StatusCode};
use runner_protocol::{NativeTaskExportContent, RunnerJob, RunnerOwner};
use serde_json::json;
use uuid::Uuid;

use crate::{
    runtime::session_gate::SessionGate,
    upstream::dispatch::native_execution::native_task_execution::support::ManagerRig,
};

pub(crate) async fn prepared(files: &[(&str, &str)]) -> ManagerRig {
    ManagerRig::prepared(files).await
}

pub(crate) async fn start(rig: &ManagerRig, token: &str) {
    let scheduled = rig.schedule("start", 4, token).await;
    assert_eq!(
        scheduled.status(),
        StatusCode::OK,
        "{}",
        response_body(scheduled).await
    );
    let job = rig.poll("start_native_task").await;
    let result = rig.execute(&job).await.expect("manager start dispatch");
    assert_eq!(
        rig.report_success(&job, result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "running");
}

pub(crate) async fn stop(rig: &ManagerRig, token: &str) -> Uuid {
    rig.fake.set_mode("require-stop-barrier");
    let scheduled = rig.schedule("stop", 4, token).await;
    assert_eq!(
        scheduled.status(),
        StatusCode::OK,
        "{}",
        response_body(scheduled).await
    );
    let stop_job = rig.poll("stop_native_task").await;
    let stop_job_id = wire_uuid(&stop_job);
    let stop_result = rig.execute(&stop_job).await.expect("manager stop dispatch");
    assert_eq!(stop_result["preparation_quiescent"], true);
    assert!(matches!(
        stop_result["environment"]["observed_state"].as_str(),
        Some("absent" | "exited" | "dead")
    ));
    assert_eq!(
        rig.report_success(&stop_job, stop_result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "stopping");
    stop_job_id
}

pub(crate) async fn schedule_export(
    rig: &ManagerRig,
    stop_job_id: Uuid,
    operation_token: &str,
    paths: &[&str],
) -> Response {
    rig.fixture
        .client
        .post(format!(
            "{}/api/internal/execution-environments/{}/tasks/{}/export",
            rig.fixture.base, rig.env_id, rig.task_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .json(&json!({
            "operation_token": operation_token,
            "stop_job_id": stop_job_id,
            "paths": paths,
        }))
        .send()
        .await
        .expect("schedule export")
}

struct StateDirGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl StateDirGuard {
    fn for_rig(rig: &ManagerRig) -> Self {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        const NAME: &str = "RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR";
        let previous = std::env::var_os(NAME);
        let state_dir = rig
            .fixture
            .root
            .parent()
            .expect("fixture root parent")
            .join("manager-state");
        // The isolated integration command is single-threaded; this guard serializes test overrides.
        unsafe { std::env::set_var(NAME, state_dir) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for StateDirGuard {
    fn drop(&mut self) {
        const NAME: &str = "RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR";
        // Restore only this test process's profile input after the backend call completes.
        unsafe {
            if let Some(value) = self.previous.take() {
                std::env::set_var(NAME, value);
            } else {
                std::env::remove_var(NAME);
            }
        }
    }
}

pub(crate) async fn dispatch_export(
    rig: &ManagerRig,
    job: RunnerJob,
) -> runner_protocol::RunnerJobResult {
    let _state_dir = StateDirGuard::for_rig(rig);
    let config = crate::upstream::native_task_preparation::support::manager_config(
        Some(rig.fixture.root.clone()),
        rig.fixture.manager_id,
    );
    config.write().await.auth_token = rig.fixture.manager_token.clone();
    let gate = SessionGate::new();
    crate::upstream::dispatch::execute_job(
        &NoopRunner,
        rig.export_backend.as_ref(),
        &rig.fixture.client,
        &rig.fixture.base,
        &config,
        &gate,
        job,
    )
    .await
}

pub(crate) async fn download(rig: &ManagerRig) -> (StatusCode, Option<NativeTaskExportContent>) {
    let response = rig
        .fixture
        .client
        .get(format!(
            "{}/api/internal/execution-environments/{}/tasks/{}/export/content",
            rig.fixture.base, rig.env_id, rig.task_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .send()
        .await
        .expect("download export");
    let status = response.status();
    let content = if status == StatusCode::OK {
        Some(response.json().await.expect("export content"))
    } else {
        None
    };
    (status, content)
}

pub(crate) async fn release(rig: &ManagerRig, stop_job_id: Uuid) -> StatusCode {
    rig.fixture
        .client
        .post(format!(
            "{}/api/internal/execution-environments/{}/tasks/{}/release",
            rig.fixture.base, rig.env_id, rig.task_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .json(&json!({
            "expected_generation": 5,
            "expected_container_generation": 4,
            "expected_stop_job_id": stop_job_id,
        }))
        .send()
        .await
        .expect("release stopped task")
        .status()
}

pub(crate) async fn replace_with_ready_successor(rig: &ManagerRig, stop_job_id: Uuid) {
    assert_eq!(release(rig, stop_job_id).await, StatusCode::OK);
    let response = rig
        .fixture
        .client
        .post(format!(
            "{}/api/internal/execution-environments/{}/replace",
            rig.fixture.base, rig.env_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .json(&json!({
            "expected_generation": 5,
            "expected_container_generation": 4,
            "expected_stop_job_id": stop_job_id,
            "operation_token": "export-ready-successor",
        }))
        .send()
        .await
        .expect("schedule successor replacement");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response_body(response).await
    );

    let job = rig.fixture.poll().await;
    let owner: RunnerOwner =
        serde_json::from_value(job.payload["owner"].clone()).expect("replacement owner");
    let result = json!({
        "owner": owner,
        "container_name": owner.container_name(),
        "state": "running",
    });
    assert_eq!(
        rig.report_success(&job, result).await,
        StatusCode::NO_CONTENT
    );
    let status_response = rig
        .fixture
        .client
        .get(format!(
            "{}/api/internal/execution-environments/{}/status",
            rig.fixture.base, rig.env_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .send()
        .await
        .expect("reconcile successor status");
    assert_eq!(status_response.status(), StatusCode::OK);
    let status_value: serde_json::Value = status_response.json().await.expect("status JSON");
    assert_eq!(status_value["state"], "ready");
    let status: (i64, String, Option<i64>, Option<Uuid>) = sqlx::query_as(
        "SELECT generation, state, container_generation, active_native_task_id \
         FROM execution_environments WHERE environment_id = $1",
    )
    .bind(rig.env_id)
    .fetch_one(&rig.fixture.db.pool)
    .await
    .expect("successor environment fence");
    assert_eq!(status.0, 5);
    assert_eq!(status.1, "ready");
}

pub(crate) async fn environment_snapshot(
    rig: &ManagerRig,
) -> (i64, String, Option<i64>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT generation, state, container_generation, active_native_task_id \
         FROM execution_environments WHERE environment_id = $1",
    )
    .bind(rig.env_id)
    .fetch_one(&rig.fixture.db.pool)
    .await
    .expect("environment snapshot")
}

pub(crate) async fn export_header_count(rig: &ManagerRig) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM execution_environment_task_exports WHERE task_id = $1")
        .bind(rig.task_id)
        .fetch_one(&rig.fixture.db.pool)
        .await
        .expect("export header count")
}

pub(crate) async fn export_file_count(rig: &ManagerRig, job: &RunnerJob) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_task_export_files WHERE job_id = $1",
    )
    .bind(wire_uuid(job))
    .fetch_one(&rig.fixture.db.pool)
    .await
    .expect("export file count")
}

pub(crate) fn wire_uuid(job: &RunnerJob) -> Uuid {
    Uuid::parse_str(
        job.job_id
            .strip_prefix("envjob:")
            .expect("environment job id"),
    )
    .expect("job UUID")
}

pub(crate) async fn response_body(response: Response) -> String {
    response.text().await.unwrap_or_default()
}
