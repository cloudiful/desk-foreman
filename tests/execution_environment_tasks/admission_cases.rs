//! Admission isolation, replay and exclusivity cases (issue 753 P1).
//!
//! Split from the suite entry so no test file exceeds the size budget. These
//! cover who may admit a task against which workspace: application and
//! subject scoping, the fixed resource marker, token replay, and the
//! per-environment and per-binding exclusivity rules.

use uuid::Uuid;

use super::support::TaskFixture;

#[tokio::test]
async fn an_admitted_task_records_identity_binding_and_state_only() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let outcome = fixture.admit_default(task_id, "op-1").await;
    assert_eq!(outcome.status, 200, "body: {}", outcome.body_text());
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(body["task_id"], task_id.to_string());
    assert_eq!(body["workspace_binding_id"], fixture.binding_id as i64);
    assert_eq!(body["target_generation"], 4);
    assert_eq!(body["state"], "admitted");
    assert_eq!(body["active"], true);
    assert_eq!(body["released"], false);
    assert_eq!(
        body["container_generation"],
        serde_json::Value::Null,
        "P1 never sets a candidate"
    );
    // The response shape is the whole contract: identity, binding, generation
    // and state. No manager, host path, image, port, state volume, credential
    // or subject echo may appear.
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "active",
            "container_generation",
            "environment_id",
            "released",
            "start_job_id",
            "state",
            "stop_job_id",
            "target_generation",
            "task_id",
            "workspace_binding_id",
        ],
        "a task descriptor must expose nothing else"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_identical_token_replay_returns_the_same_task() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let first = fixture.admit_default(task_id, "op-replay").await;
    assert_eq!(first.status, 200);
    let second = fixture.admit_default(task_id, "op-replay").await;
    assert_eq!(second.status, 200, "an identical replay must be idempotent");
    assert_eq!(first.body, second.body, "replay returns the same task");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_token_reused_for_a_different_intent_conflicts() {
    let fixture = TaskFixture::setup().await;
    let first = fixture.admit_default(Uuid::new_v4(), "op-reused").await;
    assert_eq!(first.status, 200);
    let second = fixture.admit_default(Uuid::new_v4(), "op-reused").await;
    assert_eq!(second.status, 409, "one token may name exactly one intent");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_token_reveals_nothing_and_still_admits_its_own_task() {
    let fixture = TaskFixture::setup().await;
    // Another application's task exists under this token.
    fixture.admit_default(Uuid::new_v4(), "shared-token").await;
    let other_task = Uuid::new_v4();
    let outcome = fixture
        .admit(
            &fixture.other_application_token,
            other_task,
            fixture.binding_id,
            "task-user",
            4,
            "shared-token",
        )
        .await;
    // The foreign token is invisible, so this caller is refused on its own
    // subject, not told that the token exists.
    assert_ne!(outcome.status, 200);
    assert!(
        !outcome.body_text().contains("shared-token"),
        "a refusal must not echo the caller's own token either"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_subject_that_is_not_the_environments_is_refused() {
    let fixture = TaskFixture::setup().await;
    let outcome = fixture
        .admit(
            &fixture.application_token,
            Uuid::new_v4(),
            fixture.binding_id,
            "somebody-else",
            4,
            "op-subject",
        )
        .await;
    assert_eq!(
        outcome.status, 409,
        "the subject must match the environment"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn the_fixed_resource_marker_is_accepted_and_never_compared() {
    let fixture = TaskFixture::setup().await;
    // The binding's external user is DF's fixed shared-resource marker, which
    // is deliberately different from the environment's subject. Admission
    // succeeds because the marker is never compared to the subject.
    let marker: (String,) = sqlx::query_as(
        "SELECT external_user_id FROM workspace_bindings WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("marker");
    assert_eq!(marker.0, super::support::RESOURCE_MARKER);
    assert_ne!(
        marker.0, "task-user",
        "the marker is a different identity than the environment subject"
    );
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-marker")
            .await
            .status,
        200
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_application_cannot_admit_against_our_binding() {
    let fixture = TaskFixture::setup().await;
    let outcome = fixture
        .admit(
            &fixture.other_application_token,
            Uuid::new_v4(),
            fixture.binding_id,
            "task-user",
            4,
            "op-foreign",
        )
        .await;
    assert_eq!(
        outcome.status, 404,
        "another application must not learn this binding exists"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_stale_generation_fence_is_refused() {
    let fixture = TaskFixture::setup().await;
    let outcome = fixture
        .admit(
            &fixture.application_token,
            Uuid::new_v4(),
            fixture.binding_id,
            "task-user",
            99,
            "op-stale",
        )
        .await;
    assert_eq!(outcome.status, 409);
    fixture.teardown().await;
}
