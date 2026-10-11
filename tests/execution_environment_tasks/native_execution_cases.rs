use serde_json::{Value, json};
use uuid::Uuid;

use super::native_execution_helpers::{
    parse_job_id, poll_task_job, prepare_task, schedule_preparation_job, start_result, stop_result,
};
use super::support::{TaskFixture, active_task_of, call, create_manager};

fn operation_path(fixture: &TaskFixture, task_id: Uuid, operation: &str) -> String {
    format!("{}/{}", fixture.task_path(task_id), operation)
}

fn response_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("response JSON")
}

async fn schedule_start(
    fixture: &TaskFixture,
    task_id: Uuid,
    token: &str,
) -> super::support::HttpOutcome {
    fixture
        .json_post(
            &fixture.application_token,
            &operation_path(fixture, task_id, "start"),
            4,
            json!({"expected_generation": 4, "operation_token": token}),
        )
        .await
}

async fn prepare_and_start(fixture: &TaskFixture, task_id: Uuid) {
    prepare_task(fixture, task_id).await;
    let start = schedule_start(fixture, task_id, "task-start").await;
    assert_eq!(start.status, 200, "{}", start.body_text());
    let (job_id, job) = poll_task_job(fixture).await;
    assert_eq!(job["kind"], "start_native_task");
    assert_eq!(
        fixture
            .manager_result(job_id, true, start_result(&job))
            .await
            .status,
        204
    );
}

#[tokio::test]
async fn task_start_and_stop_require_typed_original_manager_proofs_before_release() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;

    let (first, second) = tokio::join!(
        schedule_start(&fixture, task_id, "task-start"),
        schedule_start(&fixture, task_id, "task-start"),
    );
    assert_eq!(first.status, 200, "{}", first.body_text());
    assert_eq!(second.status, 200, "{}", second.body_text());
    let first_descriptor = response_json(&first.body);
    let second_descriptor = response_json(&second.body);
    assert_eq!(first_descriptor["job_id"], second_descriptor["job_id"]);
    assert_eq!(first_descriptor["task_state"], "starting");
    assert_eq!(first_descriptor["native_task_id"], task_id.to_string());

    // The added `starting` state already blocks legacy writers before a start
    // acknowledgment can move the task to running.
    let legacy = super::lease_helpers::acquire_lease(&fixture, "during-start").await;
    assert_ne!(legacy.status, 200);

    let (start_job_id, start_job) = poll_task_job(&fixture).await;
    assert_eq!(start_job["kind"], "start_native_task");
    assert!(start_job["payload"].get("host_path").is_none());
    assert!(start_job["payload"].get("image").is_none());

    let (_foreign_manager_id, foreign_manager_token) =
        create_manager(&fixture.database.pool, "task-foreign-manager").await;
    let wrong_manager = call(
        &fixture.base,
        "POST",
        "/api/internal/runner-manager/jobs/result",
        Some(&foreign_manager_token),
        &[],
        serde_json::to_vec(&json!({
            "job_id": start_job["job_id"],
            "ok": true,
            "result": start_result(&start_job),
            "error": null
        }))
        .expect("result body")
        .as_slice(),
    )
    .await;
    assert_eq!(wrong_manager.status, 403);

    let mut bad_hash = start_result(&start_job);
    bad_hash["task"]["manifest_sha256"] = json!("0".repeat(64));
    assert_eq!(
        fixture
            .manager_result(start_job_id, true, bad_hash)
            .await
            .status,
        400
    );
    let fenced_state: (String, String) = sqlx::query_as(
        "SELECT task.state, job.state FROM execution_environment_tasks AS task \
         JOIN execution_environment_jobs AS job ON job.job_id = task.start_job_id \
         WHERE task.task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("start remains claimed after malformed proof");
    assert_eq!(
        fenced_state,
        ("starting".to_string(), "claimed".to_string())
    );

    assert_eq!(
        fixture
            .manager_result(start_job_id, true, start_result(&start_job))
            .await
            .status,
        204
    );
    let started_task = fixture
        .get(&fixture.application_token, &fixture.task_path(task_id))
        .await;
    assert_eq!(response_json(&started_task.body)["state"], "running");
    let environment_status = fixture
        .get(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/status",
                fixture.environment_id
            ),
        )
        .await;
    assert_eq!(response_json(&environment_status.body)["state"], "ready");

    // A start token is not reusable as a task-stop token.
    let collided_stop = fixture
        .json_post(
            &fixture.application_token,
            &operation_path(&fixture, task_id, "stop"),
            4,
            json!({"expected_generation": 4, "operation_token": "task-start"}),
        )
        .await;
    assert_eq!(collided_stop.status, 409);

    let scheduled_stop = fixture
        .json_post(
            &fixture.application_token,
            &operation_path(&fixture, task_id, "stop"),
            4,
            json!({"expected_generation": 4, "operation_token": "task-stop"}),
        )
        .await;
    assert_eq!(scheduled_stop.status, 200, "{}", scheduled_stop.body_text());
    let stop_descriptor = response_json(&scheduled_stop.body);
    assert_eq!(stop_descriptor["generation"], 5);
    assert_eq!(stop_descriptor["task_state"], "stopping");
    let (stop_job_id, stop_job) = poll_task_job(&fixture).await;
    assert_eq!(stop_job["kind"], "stop_native_task");
    assert_eq!(
        fixture
            .manager_result(stop_job_id, true, stop_result(&stop_job, true))
            .await
            .status,
        204
    );

    let stopped = fixture
        .get(&fixture.application_token, &fixture.task_path(task_id))
        .await;
    let stopped = response_json(&stopped.body);
    assert_eq!(stopped["state"], "stopping");
    assert_eq!(stopped["active"], true);
    assert_eq!(active_task_of(&fixture).await, Some(task_id));
    let stored_proof: (serde_json::Value,) =
        sqlx::query_as("SELECT result FROM execution_environment_jobs WHERE job_id = $1")
            .bind(stop_job_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("stored task stop proof");
    assert_eq!(stored_proof.0["preparation_quiescent"], true);

    // Stop is proof production only. The application separately releases the
    // task against the advanced authorization generation and candidate fence.
    let released = fixture.release(task_id, stop_job_id, 5, 4).await;
    assert_eq!(released.status, 200, "{}", released.body_text());
    assert_eq!(response_json(&released.body)["state"], "released");
    assert_eq!(active_task_of(&fixture).await, None);
    fixture.teardown().await;
}

#[tokio::test]
async fn stopping_during_preparation_fences_the_late_preparation_ack() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let (prepare_job_id, prepare_job) = schedule_preparation_job(&fixture, task_id).await;

    let scheduled = fixture
        .json_post(
            &fixture.application_token,
            &operation_path(&fixture, task_id, "stop"),
            4,
            json!({"expected_generation": 4, "operation_token": "task-stop-preparing"}),
        )
        .await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let descriptor = response_json(&scheduled.body);
    assert_eq!(descriptor["task_state"], "stopping");
    assert_eq!(descriptor["generation"], 5);
    let prepare_state: (String,) =
        sqlx::query_as("SELECT state FROM execution_environment_jobs WHERE job_id = $1")
            .bind(prepare_job_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("fenced preparation job");
    assert_eq!(prepare_state.0, "uncertain");
    assert_eq!(
        fixture.release(task_id, Uuid::nil(), 5, 4).await.status,
        409
    );

    let (stop_job_id, stop_job) = poll_task_job(&fixture).await;
    assert_eq!(stop_job["kind"], "stop_native_task");
    assert_eq!(
        fixture
            .manager_result(stop_job_id, true, stop_result(&stop_job, true))
            .await
            .status,
        204
    );
    let payload = &prepare_job["payload"];
    let late_prepare = fixture
        .manager_result(
            prepare_job_id,
            true,
            json!({
                "task_id": task_id,
                "manifest_sha256": payload["manifest_sha256"],
                "file_count": payload["file_count"],
                "total_bytes": payload["total_bytes"],
                "prepared": true
            }),
        )
        .await;
    assert_eq!(late_prepare.status, 409);
    assert_eq!(
        fixture.release(task_id, stop_job_id, 5, 4).await.status,
        200
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_prepared_task_stop_records_a_candidate_without_releasing_early() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let scheduled = fixture
        .json_post(
            &fixture.application_token,
            &operation_path(&fixture, task_id, "stop"),
            4,
            json!({"expected_generation": 4, "operation_token": "prepared-stop"}),
        )
        .await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let descriptor = response_json(&scheduled.body);
    assert_eq!(descriptor["generation"], 5);
    assert_eq!(descriptor["environment_state"], "stopping");
    assert_eq!(descriptor["task_state"], "stopping");
    let stop_job_id = parse_job_id(
        &json!({"job_id": format!("envjob:{}", descriptor["job_id"].as_str().unwrap())}),
    );
    assert_eq!(
        fixture.release(task_id, stop_job_id, 5, 4).await.status,
        409
    );

    let (job_id, job) = poll_task_job(&fixture).await;
    assert_eq!(job["kind"], "stop_native_task");
    assert_eq!(
        fixture
            .manager_result(job_id, true, stop_result(&job, true))
            .await
            .status,
        204
    );
    assert_eq!(fixture.release(task_id, job_id, 5, 4).await.status, 200);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_start_ack_racing_stop_cannot_resurrect_the_task() {
    for attempt in 0..3 {
        let fixture = TaskFixture::setup().await;
        let task_id = Uuid::new_v4();
        prepare_task(&fixture, task_id).await;
        let scheduled = schedule_start(&fixture, task_id, &format!("start-race-{attempt}")).await;
        assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
        let (start_job_id, start_job) = poll_task_job(&fixture).await;
        let late_start = fixture.manager_result(start_job_id, true, start_result(&start_job));
        let stop_path = operation_path(&fixture, task_id, "stop");
        let stop_request = fixture.json_post(
            &fixture.application_token,
            &stop_path,
            4,
            json!({"expected_generation": 4, "operation_token": format!("stop-race-{attempt}")}),
        );
        let (start_outcome, stop_outcome) = tokio::join!(late_start, stop_request);
        assert!(
            matches!(start_outcome.status, 204 | 409),
            "unexpected start race outcome: {}",
            start_outcome.body_text()
        );
        assert_eq!(stop_outcome.status, 200, "{}", stop_outcome.body_text());
        let stop_descriptor = response_json(&stop_outcome.body);
        assert_eq!(stop_descriptor["generation"], 5);
        assert_eq!(stop_descriptor["task_state"], "stopping");

        let (stop_job_id, stop_job) = poll_task_job(&fixture).await;
        assert_eq!(stop_job["kind"], "stop_native_task");
        assert_eq!(
            fixture
                .manager_result(stop_job_id, true, stop_result(&stop_job, true))
                .await
                .status,
            204
        );
        let task = fixture
            .get(&fixture.application_token, &fixture.task_path(task_id))
            .await;
        assert_eq!(response_json(&task.body)["state"], "stopping");
        assert_eq!(
            fixture.release(task_id, stop_job_id, 5, 4).await.status,
            200
        );
        fixture.teardown().await;
    }
}

#[tokio::test]
async fn false_preparation_quiescence_retains_task_ownership_and_cannot_release() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_and_start(&fixture, task_id).await;

    let scheduled = fixture
        .json_post(
            &fixture.application_token,
            &operation_path(&fixture, task_id, "stop"),
            4,
            json!({"expected_generation": 4, "operation_token": "task-stop-false"}),
        )
        .await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let (stop_job_id, stop_job) = poll_task_job(&fixture).await;
    let stop_ack = stop_result(&stop_job, false);
    assert_eq!(
        fixture
            .manager_result(stop_job_id, true, stop_ack)
            .await
            .status,
        204
    );
    let task = fixture
        .get(&fixture.application_token, &fixture.task_path(task_id))
        .await;
    assert_eq!(response_json(&task.body)["state"], "uncertain");
    assert_eq!(active_task_of(&fixture).await, Some(task_id));
    let environment_status = fixture
        .get(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/status",
                fixture.environment_id
            ),
        )
        .await;
    assert_eq!(
        response_json(&environment_status.body)["state"],
        "uncertain"
    );
    assert_eq!(
        fixture.release(task_id, stop_job_id, 5, 4).await.status,
        409
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn start_requires_preparation_current_generation_and_the_original_online_manager() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "task-admit").await.status,
        200
    );
    assert_eq!(
        schedule_start(&fixture, task_id, "early-start")
            .await
            .status,
        409
    );
    let state: (String, Option<Uuid>, Option<i64>) = sqlx::query_as(
        "SELECT state, start_job_id, container_generation \
         FROM execution_environment_tasks WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("admitted task remains unchanged");
    assert_eq!(state, ("admitted".to_string(), None, None));
    fixture.teardown().await;

    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let stale = fixture
        .json_post(
            &fixture.application_token,
            &operation_path(&fixture, task_id, "start"),
            3,
            json!({"expected_generation": 3, "operation_token": "stale-start"}),
        )
        .await;
    assert_eq!(stale.status, 409);

    // A second live manager is not a failover target for this task.
    let _ = create_manager(&fixture.database.pool, "task-alternate-manager").await;
    sqlx::query("UPDATE runner_managers SET status = 'offline' WHERE runner_manager_id = $1")
        .bind(fixture.manager_id)
        .execute(&fixture.database.pool)
        .await
        .expect("offline original manager");
    let unavailable = schedule_start(&fixture, task_id, "offline-start").await;
    assert_eq!(unavailable.status, 503);
    let state: (String, Option<Uuid>, Option<i64>) = sqlx::query_as(
        "SELECT state, start_job_id, container_generation \
         FROM execution_environment_tasks WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("prepared task remains unchanged");
    assert_eq!(state, ("prepared".to_string(), None, None));
    fixture.teardown().await;
}

#[tokio::test]
async fn legacy_environment_mutations_refuse_an_active_native_task() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "task-admit").await.status,
        200
    );

    let invalidated = fixture
        .json_post(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/invalidate",
                fixture.environment_id
            ),
            4,
            json!({"expected_generation": 4}),
        )
        .await;
    assert_eq!(invalidated.status, 409);
    let provisioned = fixture
        .json_post(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/provision",
                fixture.environment_id
            ),
            4,
            json!({"expected_generation": 4, "operation_token": "legacy-provision"}),
        )
        .await;
    assert_eq!(provisioned.status, 409);
    let generation: (i64, Option<i64>, Option<Uuid>) = sqlx::query_as(
        "SELECT generation, container_generation, active_native_task_id \
         FROM execution_environments WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("environment fences");
    assert_eq!(generation.0, 4);
    assert_eq!(generation.1, None);
    assert_eq!(generation.2, Some(task_id));
    fixture.teardown().await;
}
