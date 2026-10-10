//! Candidate-shape admission cases (issue 753 P1).
//!
//! Split from `admission_cases.rs` so no test file exceeds the size budget.
//! An environment admits a task in exactly two shapes: no candidate at all, or
//! a candidate a confirmed stop from its own immutable manager released. A
//! live `ready`/`running` candidate is refused, never silently adopted.

use uuid::Uuid;

use super::support::{TaskFixture, confirmed_stop};

async fn seed_replaced_candidate(
    fixture: &TaskFixture,
    proof_result: serde_json::Value,
    proof_state: &str,
    proof_manager_id: i64,
) -> Uuid {
    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, claimed_at, deadline_at, \
             terminal_at, result \
         ) VALUES ($1, $2, $3, $4, $5, 'stop', 4, $6, NOW(), NOW(), NOW(), $7)",
    )
    .bind(job_id)
    .bind(format!("replaced-proof-{job_id}"))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(proof_manager_id)
    .bind(proof_state)
    .bind(proof_result)
    .execute(&fixture.database.pool)
    .await
    .expect("insert candidate stop proof");

    sqlx::query(
        "UPDATE execution_environments SET container_generation = 4, state = 'replaced', \
         stop_proof_job_id = $2, stop_proof_generation = 4 WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .bind(job_id)
    .execute(&fixture.database.pool)
    .await
    .expect("mark environment replaced");
    job_id
}

#[tokio::test]
async fn a_confirmed_stop_from_the_original_manager_admits_on_a_replaced_candidate() {
    let fixture = TaskFixture::setup().await;
    seed_replaced_candidate(&fixture, confirmed_stop(), "succeeded", fixture.manager_id).await;
    let outcome = fixture.admit_default(Uuid::new_v4(), "op-replaced").await;
    assert_eq!(
        outcome.status,
        200,
        "a verified replaced candidate is admissible: {}",
        outcome.body_text()
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_ready_or_running_candidate_is_never_admitted() {
    let fixture = TaskFixture::setup().await;
    // A live container whose mounts this phase cannot reason about.
    sqlx::query(
        "UPDATE execution_environments SET container_generation = 4, state = 'ready' \
         WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .execute(&fixture.database.pool)
    .await
    .expect("mark environment ready");
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-ready")
            .await
            .status,
        409,
        "a ready candidate must be refused, not silently adopted"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_candidate_without_a_verified_stop_proof_is_refused() {
    for (proof_result, proof_state, manager_offset) in [
        // Unconfirmed observation.
        (
            serde_json::json!({"outcome": "stop_confirmed", "observed_state": "exited", "confirmed": false}),
            "succeeded",
            0,
        ),
        // Failed stop.
        (
            serde_json::json!({"outcome": "stop_failed", "observed_state": null, "confirmed": false}),
            "failed",
            0,
        ),
        // A confirmation from some other manager.
        (
            serde_json::json!({"outcome": "stop_confirmed", "observed_state": "exited", "confirmed": true}),
            "succeeded",
            1000,
        ),
    ] {
        let fixture = TaskFixture::setup().await;
        let proof_manager = if manager_offset == 0 {
            fixture.manager_id
        } else {
            super::support::create_manager(&fixture.database.pool, "replaced-other-manager")
                .await
                .0
        };
        seed_replaced_candidate(&fixture, proof_result, proof_state, proof_manager).await;
        let outcome = fixture.admit_default(Uuid::new_v4(), "op-unproven").await;
        assert_eq!(
            outcome.status,
            409,
            "an unproven candidate must never admit: {}",
            outcome.body_text()
        );
        fixture.teardown().await;
    }
}

#[tokio::test]
async fn a_candidate_whose_proof_targets_another_generation_is_refused() {
    let fixture = TaskFixture::setup().await;
    let job_id =
        seed_replaced_candidate(&fixture, confirmed_stop(), "succeeded", fixture.manager_id).await;
    // The proof must be for the candidate the environment carries.
    sqlx::query(
        "UPDATE execution_environments SET stop_proof_generation = 99 WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .execute(&fixture.database.pool)
    .await
    .expect("drift proof generation");
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-gen-drift")
            .await
            .status,
        409
    );
    // The proof row itself is still the same confirmed row, so the refusal is
    // about the mismatch, not about the proof being absent.
    let stored: (Option<Uuid>,) = sqlx::query_as(
        "SELECT stop_proof_job_id FROM execution_environments WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("proof anchor");
    assert_eq!(stored.0, Some(job_id));
    fixture.teardown().await;
}
