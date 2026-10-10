//! Cancel cases (issue 753 P1).
//!
//! Split from the original combined file so no test file exceeds the size
//! budget. Cancel is a decision about proof: a task that provably never started
//! is voided and frees its environment, anything else becomes uncertain and
//! keeps blocking across every generation.

use uuid::Uuid;

use super::support::{TaskFixture, active_task_of, call, fence_header, stored_state};

#[tokio::test]
async fn cancelling_a_never_started_task_voids_it_and_frees_the_environment() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "op-cancel").await.status,
        200
    );

    let outcome = fixture
        .json_post(
            &fixture.application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    assert_eq!(outcome.status, 200, "body: {}", outcome.body_text());
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(body["state"], "released", "a voided task is terminal");
    assert_eq!(body["released"], true);
    assert_eq!(
        active_task_of(&fixture).await,
        None,
        "a voided task frees its environment"
    );
    // The workspace is free again for a new intent.
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-after-cancel")
            .await
            .status,
        200
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn cancelling_a_task_with_a_dispatch_reference_is_uncertain_and_keeps_blocking() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "op-uncertain").await.status,
        200
    );

    // A durable job now references this task: something may have been
    // dispatched, so it cannot be voided as never-started.
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, native_task_id, deadline_at \
         ) VALUES ($1, $2, $3, $4, $5, 'ensure', 4, 'pending', $6, NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(format!("job-{}", task_id))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(fixture.manager_id)
    .bind(task_id)
    .execute(&fixture.database.pool)
    .await
    .expect("insert referenced job");

    let outcome = fixture
        .json_post(
            &fixture.application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    assert_eq!(outcome.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(
        body["state"], "uncertain",
        "a possibly-dispatched task is uncertain, never voided"
    );
    assert_eq!(
        active_task_of(&fixture).await,
        Some(task_id),
        "an uncertain task keeps holding the environment"
    );
    assert_eq!(
        stored_state(&fixture, task_id).await,
        "uncertain",
        "uncertainty is durable, not a response-only label"
    );
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-blocked")
            .await
            .status,
        409,
        "an uncertain task blocks a new task across generations"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn cancelling_twice_is_idempotent() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-twice").await;
    let first = fixture
        .json_post(
            &fixture.application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    let second = fixture
        .json_post(
            &fixture.application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    assert_eq!(first.body, second.body, "a repeated cancel changes nothing");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_cancel_from_another_application_is_absent() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-owner").await;
    let outcome = fixture
        .json_post(
            &fixture.other_application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    assert_eq!(outcome.status, 404, "a foreign task must not be revealed");
    assert_eq!(
        stored_state(&fixture, task_id).await,
        "admitted",
        "a refused cancel must not mutate the owner's task"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_task_read_returns_only_its_own_state() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-read").await;
    let own = fixture
        .get(&fixture.application_token, &fixture.task_path(task_id))
        .await;
    assert_eq!(own.status, 200);
    assert!(own.body_text().contains("\"state\":\"admitted\""));

    let foreign = fixture
        .get(
            &fixture.other_application_token,
            &fixture.task_path(task_id),
        )
        .await;
    assert_eq!(foreign.status, 404, "a task is visible only to its owner");
    let missing = fixture
        .get(
            &fixture.application_token,
            &fixture.task_path(Uuid::new_v4()),
        )
        .await;
    assert_eq!(missing.status, 404);
    fixture.teardown().await;
}

#[tokio::test]
async fn an_admit_without_a_bearer_is_refused() {
    let fixture = TaskFixture::setup().await;
    let outcome = call(
        &fixture.base,
        "POST",
        &fixture.admit_path(),
        None,
        &[fence_header(4)],
        serde_json::to_vec(&serde_json::json!({
            "task_id": Uuid::new_v4(),
            "workspace_binding_id": fixture.binding_id,
            "subject_external_user_id": "task-user",
            "expected_generation": 4,
            "operation_token": "op-nobearer",
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_eq!(outcome.status, 401);
    fixture.teardown().await;
}
