use reqwest::StatusCode;
use runner_protocol::RunnerJob;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::ManagerRig;

const FILES: &[(&str, &str)] = &[
    ("src/main.rs", "fn main() {}\n"),
    ("README.md", "initial\n"),
];

fn wire_uuid(job: &RunnerJob) -> Uuid {
    Uuid::parse_str(
        job.job_id
            .strip_prefix("envjob:")
            .expect("durable environment job id"),
    )
    .expect("job UUID")
}

async fn release(
    rig: &ManagerRig,
    expected_generation: i64,
    expected_container_generation: i64,
    stop_job_id: Uuid,
) -> reqwest::Response {
    rig.fixture
        .client
        .post(format!(
            "{}/api/internal/execution-environments/{}/tasks/{}/release",
            rig.fixture.base, rig.env_id, rig.task_id
        ))
        .bearer_auth(&rig.fixture.application_token)
        .header("X-DF-External-User-Id", "prep-user")
        .json(&json!({
            "expected_generation": expected_generation,
            "expected_container_generation": expected_container_generation,
            "expected_stop_job_id": stop_job_id,
        }))
        .send()
        .await
        .expect("release task")
}

async fn start(rig: &ManagerRig) -> (RunnerJob, Value) {
    let scheduled = rig.schedule("start", 4, "wire-start").await;
    assert_eq!(
        scheduled.status(),
        StatusCode::OK,
        "{}",
        scheduled.text().await.expect("schedule response body")
    );
    let job = rig.poll("start_native_task").await;
    let result = rig.execute(&job).await.expect("manager start dispatch");
    assert_eq!(
        rig.report_success(&job, result.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "running");
    let (candidate, state) = rig.fixture.environment_fence().await;
    assert_eq!(candidate, Some(4));
    assert_eq!(state, "ready");
    (job, result)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory named gate"]
async fn native_task_start_stop_release_runs_through_router_and_manager_dispatch() {
    let rig = ManagerRig::prepared(FILES).await;
    let (_start_job, start_result) = start(&rig).await;
    assert_eq!(start_result["task"]["task_id"], rig.task_id.to_string());
    assert_eq!(start_result["environment"]["state"], "running");

    let edited = rig.tree_file("README.md");
    std::fs::write(&edited, "edited by native task\n").expect("task edit");
    rig.fake.set_mode("require-stop-barrier");
    let scheduled = rig.schedule("stop", 4, "wire-stop").await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let stop_job = rig.poll("stop_native_task").await;
    let stop_result = rig.execute(&stop_job).await.expect("manager stop dispatch");
    assert_eq!(stop_result["task"]["task_id"], rig.task_id.to_string());
    assert_eq!(stop_result["preparation_quiescent"], true);
    assert_eq!(stop_result["environment"]["confirmed"], true);
    assert!(matches!(
        stop_result["environment"]["observed_state"].as_str(),
        Some("absent" | "exited" | "dead")
    ));
    assert_eq!(
        rig.report_success(&stop_job, stop_result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "stopping");
    let (candidate, state) = rig.fixture.environment_fence().await;
    assert_eq!(candidate, Some(4));
    assert_eq!(state, "replaced");
    assert_eq!(rig.fake.stop_count(), 1);
    assert_eq!(
        std::fs::read_to_string(edited).expect("edited checkout retained"),
        "edited by native task\n"
    );

    let released = release(&rig, 5, 4, wire_uuid(&stop_job)).await;
    assert_eq!(released.status(), StatusCode::OK);
    assert_eq!(rig.fixture.task_state().await, "released");
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory named gate"]
async fn a_cold_environment_binds_its_original_manager_at_the_native_start() {
    let rig = ManagerRig::prepared_cold(FILES).await;
    // The positive public prepare acknowledgment has already run here, and the
    // environment is still unbound: no fixture wrote a manager onto it.
    assert_eq!(
        rig.environment_manager().await,
        None,
        "preparation must leave a cold environment unbound"
    );

    let (start_job, start_result) = start(&rig).await;
    assert_eq!(start_result["task"]["task_id"], rig.task_id.to_string());
    assert_eq!(start_result["environment"]["state"], "running");
    assert_eq!(
        rig.environment_manager().await,
        Some(rig.fixture.manager_id),
        "the native start binds the task's own original manager"
    );

    rig.fake.set_mode("require-stop-barrier");
    let scheduled = rig.schedule("stop", 4, "cold-stop").await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let stop_job = rig.poll("stop_native_task").await;
    let stop_result = rig.execute(&stop_job).await.expect("manager stop dispatch");
    assert_eq!(stop_result["preparation_quiescent"], true);
    assert_eq!(
        rig.report_success(&stop_job, stop_result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "stopping");

    let released = release(&rig, 5, 4, wire_uuid(&stop_job)).await;
    assert_eq!(released.status(), StatusCode::OK);
    assert_eq!(rig.fixture.task_state().await, "released");
    assert_eq!(
        rig.environment_manager().await,
        Some(rig.fixture.manager_id),
        "the binding survives the task"
    );
    let _ = start_job;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory named gate"]
async fn prepared_task_stop_proves_absence_before_release_without_creating_or_cleaning_tree() {
    let rig = ManagerRig::prepared(FILES).await;
    let before = std::fs::read(rig.tree_file("README.md")).expect("seed checkout");
    let scheduled = rig.schedule("stop", 4, "prepared-stop").await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let stop_job = rig.poll("stop_native_task").await;
    let stop_result = rig
        .execute(&stop_job)
        .await
        .expect("quiescent prepared stop");
    assert_eq!(stop_result["preparation_quiescent"], true);
    assert_eq!(stop_result["environment"]["observed_state"], "absent");
    assert_eq!(
        rig.report_success(&stop_job, stop_result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fake.stop_count(), 0);
    assert_eq!(
        std::fs::read(rig.tree_file("README.md")).expect("checkout retained"),
        before
    );
    let released = release(&rig, 5, 4, wire_uuid(&stop_job)).await;
    assert_eq!(released.status(), StatusCode::OK);
    assert_eq!(rig.fixture.task_state().await, "released");
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory named gate"]
async fn a_stop_result_without_preparation_quiescence_cannot_release_the_task() {
    let rig = ManagerRig::prepared(FILES).await;
    start(&rig).await;
    let scheduled = rig.schedule("stop", 4, "false-quiescence-stop").await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let stop_job = rig.poll("stop_native_task").await;
    let mut stop_result = rig.execute(&stop_job).await.expect("manager stop dispatch");
    stop_result["preparation_quiescent"] = json!(false);
    assert_eq!(
        rig.report_success(&stop_job, stop_result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "uncertain");
    let denied = release(&rig, 5, 4, wire_uuid(&stop_job)).await;
    assert_eq!(denied.status(), StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory named gate"]
async fn uncertain_start_with_a_candidate_can_be_stopped_and_released_only_after_proof() {
    let rig = ManagerRig::prepared(FILES).await;
    rig.fake.set_mode("fail-task-probe");
    let scheduled = rig.schedule("start", 4, "uncertain-start").await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let start_job = rig.poll("start_native_task").await;
    assert!(rig.execute(&start_job).await.is_err());
    assert_eq!(
        rig.report_failure(&start_job, "native task workspace probe failed")
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "uncertain");
    assert!(rig.task_tree.is_dir());

    rig.fake.set_mode("require-stop-barrier");
    let scheduled = rig.schedule("stop", 4, "recover-uncertain-start").await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let stop_job = rig.poll("stop_native_task").await;
    let stop_result = rig
        .execute(&stop_job)
        .await
        .expect("stop uncertain candidate");
    assert_eq!(stop_result["preparation_quiescent"], true);
    assert_eq!(
        rig.report_success(&stop_job, stop_result).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, "stopping");
    let released = release(&rig, 5, 4, wire_uuid(&stop_job)).await;
    assert_eq!(released.status(), StatusCode::OK);
    assert_eq!(rig.fixture.task_state().await, "released");
}
