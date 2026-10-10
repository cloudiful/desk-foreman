//! Task seeding and read helpers for the native-task suite (issue 753 P1).
//!
//! Split from `db_fixtures.rs` so each file stays inside the size budget. P1
//! has no dispatcher, so a release can only be exercised against a fixture
//! that already proves an executed task — those seeds live here.

use uuid::Uuid;

use super::support::TaskFixture;

pub async fn stored_state(fixture: &TaskFixture, task_id: Uuid) -> String {
    let row: (String,) =
        sqlx::query_as("SELECT state FROM execution_environment_tasks WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("stored task");
    row.0
}

pub async fn active_task_of(fixture: &TaskFixture) -> Option<Uuid> {
    let row: (Option<Uuid>,) = sqlx::query_as(
        "SELECT active_native_task_id FROM execution_environments WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("environment");
    row.0
}

/// Seed an executed task: P1 has no dispatcher, so a release can only be
/// exercised against a fixture that already proved a container ran.
pub async fn seed_executed_task(
    fixture: &TaskFixture,
    task_id: Uuid,
    proof_result: serde_json::Value,
) -> Uuid {
    sqlx::query(
        "UPDATE execution_environment_tasks \
         SET state = 'running', container_generation = 4 \
         WHERE task_id = $1",
    )
    .bind(task_id)
    .execute(&fixture.database.pool)
    .await
    .expect("mark task running");
    sqlx::query(
        "UPDATE execution_environments \
         SET container_generation = 4, state = 'stopping' \
         WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .execute(&fixture.database.pool)
    .await
    .expect("mark environment stopping");

    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, native_task_id, claimed_at, \
             deadline_at, terminal_at, result \
         ) VALUES ($1, $2, $3, $4, $5, 'stop', 4, 'succeeded', $6, NOW(), \
                   NOW(), NOW(), $7)",
    )
    .bind(job_id)
    .bind(format!("proof-{job_id}"))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(fixture.manager_id)
    .bind(task_id)
    .bind(proof_result)
    .execute(&fixture.database.pool)
    .await
    .expect("insert stop proof");
    sqlx::query(
        "UPDATE execution_environments \
         SET stop_proof_job_id = $2, stop_proof_generation = 4 \
         WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .bind(job_id)
    .execute(&fixture.database.pool)
    .await
    .expect("record consumed proof");
    job_id
}

pub fn confirmed_stop() -> serde_json::Value {
    serde_json::json!({
        "outcome": "stop_confirmed",
        "observed_state": "exited",
        "confirmed": true
    })
}
