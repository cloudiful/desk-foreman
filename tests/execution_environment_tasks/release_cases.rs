//! Release cases (issue 753 P1).
//!
//! Split from the original combined file so no test file exceeds the size
//! budget. Release accepts exactly one thing: a confirmed stop proof that
//! belongs to this task, ran on this task's original manager, targeted this
//! task's generation and records positive quiescence.

use uuid::Uuid;

use super::support::{
    TaskFixture, active_task_of, confirmed_stop, seed_executed_task, stored_state,
};

#[tokio::test]
async fn a_matching_confirmed_stop_releases_the_task_exactly_once() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-release").await;
    let proof = seed_executed_task(&fixture, task_id, confirmed_stop()).await;

    let first = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(first.status, 200, "body: {}", first.body_text());
    let body: serde_json::Value = serde_json::from_slice(&first.body).expect("json");
    assert_eq!(body["state"], "released");
    assert_eq!(active_task_of(&fixture).await, None);

    // Idempotent: a repeated release returns the same terminal task.
    let second = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(second.status, 200);
    assert_eq!(first.body, second.body);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_wrong_proof_manager_or_generation_never_releases() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-wrongproof").await;
    let proof = seed_executed_task(&fixture, task_id, confirmed_stop()).await;

    // Wrong container generation.
    let wrong_generation = fixture.release(task_id, proof, 4, 99).await;
    assert_eq!(wrong_generation.status, 409);

    // Unknown proof id.
    let unknown_proof = fixture.release(task_id, Uuid::new_v4(), 4, 4).await;
    assert_eq!(unknown_proof.status, 409);

    // A proof from a different manager.
    let (other_manager_id, _) =
        super::support::create_manager(&fixture.database.pool, "other-task-manager").await;
    sqlx::query("UPDATE execution_environment_jobs SET assigned_manager_id = $2 WHERE job_id = $1")
        .bind(proof)
        .bind(other_manager_id)
        .execute(&fixture.database.pool)
        .await
        .expect("rewire proof manager");
    let wrong_manager = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(wrong_manager.status, 409);

    // A proof that belongs to a different real task: the typed link is an FK,
    // so a foreign or fabricated task id cannot even be stored.
    let other_task = seed_other_released_task(&fixture, proof).await;
    sqlx::query("UPDATE execution_environment_jobs SET assigned_manager_id = $2, native_task_id = $3 WHERE job_id = $1")
        .bind(proof)
        .bind(fixture.manager_id)
        .bind(other_task)
        .execute(&fixture.database.pool)
        .await
        .expect("relink proof task");
    let wrong_task = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(wrong_task.status, 409);

    assert_eq!(
        stored_state(&fixture, task_id).await,
        "running",
        "no refused release may touch the task"
    );
    assert_eq!(active_task_of(&fixture).await, Some(task_id));
    fixture.teardown().await;
}

#[tokio::test]
async fn a_failed_or_unconfirmed_proof_never_releases() {
    for (state, result) in [
        (
            "failed",
            serde_json::json!({"outcome": "stop_failed", "observed_state": null, "confirmed": false}),
        ),
        (
            "succeeded",
            serde_json::json!({"outcome": "stop_confirmed", "observed_state": "exited", "confirmed": false}),
        ),
        (
            "succeeded",
            serde_json::json!({"outcome": "ensure_running", "observed_state": "running", "confirmed": true}),
        ),
    ] {
        let fixture = TaskFixture::setup().await;
        let task_id = Uuid::new_v4();
        fixture.admit_default(task_id, "op-badproof").await;
        let proof = seed_executed_task(&fixture, task_id, result).await;
        sqlx::query("UPDATE execution_environment_jobs SET state = $2 WHERE job_id = $1")
            .bind(proof)
            .bind(state)
            .execute(&fixture.database.pool)
            .await
            .expect("set proof state");
        let outcome = fixture
            .json_post(
                &fixture.application_token,
                &fixture.release_path(task_id),
                4,
                serde_json::json!({
                    "expected_generation": 4,
                    "expected_container_generation": 4,
                    "expected_stop_job_id": proof,
                }),
            )
            .await;
        assert_eq!(
            outcome.status, 409,
            "a {state} proof with the given result must never release"
        );
        assert_eq!(stored_state(&fixture, task_id).await, "running");
        fixture.teardown().await;
    }
}

/// A second, already-released task row for the same environment.
///
/// Release must reject a proof that belongs to some other task, so the probe
/// needs a real task to point at: the typed link is a foreign key, and a
/// released row is outside the active partial unique indexes.
async fn seed_other_released_task(fixture: &TaskFixture, proof: Uuid) -> Uuid {
    let other_task = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO execution_environment_tasks ( \
             task_id, operation_token, environment_id, application_id, \
             workspace_binding_id, subject_external_user_id, target_generation, \
             assigned_manager_id, state, container_generation, terminal_at \
         ) VALUES ($1, $2, $3, $4, $5, 'other-subject', 4, $6, 'released', 4, NOW())",
    )
    .bind(other_task)
    .bind(format!("other-{proof}"))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(fixture.binding_id)
    .bind(fixture.manager_id)
    .execute(&fixture.database.pool)
    .await
    .expect("seed another released task");
    other_task
}
