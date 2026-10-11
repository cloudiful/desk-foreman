use uuid::Uuid;

use super::support::TaskFixture;

async fn insert_job(
    fixture: &TaskFixture,
    job_kind: &str,
    native_task_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, deadline_at, terminal_at, native_task_id \
         ) VALUES ($1, $2, $3, $4, $5, $6, 4, 'failed', NOW() + INTERVAL '5 minutes', NOW(), $7)",
    )
    .bind(Uuid::new_v4())
    .bind(format!("job-{}", Uuid::new_v4()))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(fixture.manager_id)
    .bind(job_kind)
    .bind(native_task_id)
    .execute(&fixture.database.pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn native_job_kinds_require_task_links_without_restricting_legacy_jobs() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture
            .admit_default(task_id, "constraint-task")
            .await
            .status,
        200
    );

    for job_kind in ["prepare_native_task", "start_native_task"] {
        let error = insert_job(&fixture, job_kind, None)
            .await
            .expect_err("native execution jobs must carry a task link");
        assert!(matches!(
            error,
            sqlx::Error::Database(ref database_error)
                if database_error.code().as_deref() == Some("23514")
        ));
        insert_job(&fixture, job_kind, Some(task_id))
            .await
            .expect("native execution jobs accept their task link");
    }

    for job_kind in ["ensure", "stop"] {
        insert_job(&fixture, job_kind, None)
            .await
            .expect("legacy jobs may omit a task link");
        insert_job(&fixture, job_kind, Some(task_id))
            .await
            .expect("legacy jobs may preserve an optional task link");
    }

    fixture.teardown().await;
}
