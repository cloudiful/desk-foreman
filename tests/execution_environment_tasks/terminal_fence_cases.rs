//! Terminal-release fence regressions (issue 753 P1r).
//!
//! An already-released task must still pass every fence before it is returned:
//! the caller's expected generation, the task's container generation, the
//! environment's retained candidate and the matching retained proof. An exact
//! proof replay under a matching fence is idempotent; a stale generation or a
//! malformed proof is refused even though the task is already terminal.

use uuid::Uuid;

use super::support::{
    TaskFixture, active_task_of, confirmed_stop, seed_executed_task, stored_state,
};

#[tokio::test]
async fn an_exact_terminal_replay_succeeds_while_the_fence_matches() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-terminal").await;
    let proof = seed_executed_task(&fixture, task_id, confirmed_stop()).await;

    let first = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(first.status, 200, "body: {}", first.body_text());
    assert_eq!(active_task_of(&fixture).await, None);
    assert_eq!(stored_state(&fixture, task_id).await, "released");

    // Same proof, same fences: an exact replay returns the same terminal task.
    let replay = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(
        replay.status,
        200,
        "an exact terminal replay must stay idempotent: {}",
        replay.body_text()
    );
    assert_eq!(first.body, replay.body);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_stale_generation_terminal_replay_is_refused() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-terminal-stale").await;
    let proof = seed_executed_task(&fixture, task_id, confirmed_stop()).await;
    assert_eq!(fixture.release(task_id, proof, 4, 4).await.status, 200);

    // The environment has since advanced: the old generation no longer matches
    // even though the task is terminal and the proof is unchanged.
    sqlx::query("UPDATE execution_environments SET generation = 5 WHERE environment_id = $1")
        .bind(fixture.environment_id)
        .execute(&fixture.database.pool)
        .await
        .expect("advance generation");
    let stale = fixture.release(task_id, proof, 4, 4).await;
    assert_eq!(
        stale.status,
        409,
        "a stale termination fence must be refused: {}",
        stale.body_text()
    );
    assert_eq!(stored_state(&fixture, task_id).await, "released");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_malformed_terminal_replay_is_refused() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture
        .admit_default(task_id, "op-terminal-malformed")
        .await;
    let proof = seed_executed_task(&fixture, task_id, confirmed_stop()).await;
    assert_eq!(fixture.release(task_id, proof, 4, 4).await.status, 200);

    // A wrong container generation, an unknown proof id and a proof that no
    // longer matches are all refused after the first release.
    assert_eq!(fixture.release(task_id, proof, 4, 99).await.status, 409);
    assert_eq!(
        fixture.release(task_id, Uuid::new_v4(), 4, 4).await.status,
        409
    );

    // Break the retained proof's evidence: it is no longer a confirmed stop.
    sqlx::query("UPDATE execution_environment_jobs SET result = $2 WHERE job_id = $1")
        .bind(proof)
        .bind(serde_json::json!({
            "outcome": "stop_confirmed",
            "observed_state": "exited",
            "confirmed": false
        }))
        .execute(&fixture.database.pool)
        .await
        .expect("weaken retained proof");
    assert_eq!(fixture.release(task_id, proof, 4, 4).await.status, 409);

    // None of the refused replays disturbed the terminal row.
    assert_eq!(stored_state(&fixture, task_id).await, "released");
    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_application_terminal_replay_is_absent() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-terminal-foreign").await;
    let proof = seed_executed_task(&fixture, task_id, confirmed_stop()).await;
    assert_eq!(fixture.release(task_id, proof, 4, 4).await.status, 200);

    // Another application cannot see the terminal task at all, and the refusal
    // does not confirm it exists.
    let path = fixture.release_path(task_id);
    let foreign = fixture
        .json_post(
            &fixture.other_application_token,
            &path,
            4,
            serde_json::json!({
                "expected_generation": 4,
                "expected_container_generation": 4,
                "expected_stop_job_id": proof,
            }),
        )
        .await;
    assert_eq!(foreign.status, 404, "body: {}", foreign.body_text());
    fixture.teardown().await;
}
