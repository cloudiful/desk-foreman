//! Shared helpers for the native-task preparation suite (issue 753 P2b).

use uuid::Uuid;

use super::support::{TaskFixture, seed_files};

pub async fn admitted_and_seeded(fixture: &TaskFixture, token: &str) -> Uuid {
    let task_id = Uuid::new_v4();
    assert_eq!(fixture.admit_default(task_id, token).await.status, 200);
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a"), ("b.txt", "b")]),
            )
            .await
            .status,
        200
    );
    task_id
}

/// The preparation job row for one task.
pub async fn prepare_job(fixture: &TaskFixture, task_id: Uuid) -> (Uuid, String, Option<Uuid>) {
    sqlx::query_as(
        "SELECT job_id, state, native_task_id FROM execution_environment_jobs \
         WHERE native_task_id = $1 AND job_kind = 'prepare_native_task'",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("prepare job row")
}

pub async fn task_state(fixture: &TaskFixture, task_id: Uuid) -> String {
    let row: (String,) =
        sqlx::query_as("SELECT state FROM execution_environment_tasks WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("task state");
    row.0
}
