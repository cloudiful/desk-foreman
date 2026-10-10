//! Shared fixtures for the environment-job cases.
//!
//! Only job-specific seeding lives here; the isolated database and the
//! application/token helper are reused from the execution-environment harness.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use desk_foreman::db::queries::{
    EnqueueOutcome, EnsureOutcome, enqueue_execution_environment_job, ensure_execution_environment,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Insert a runner manager row (enabled or disabled) and return its id.
pub async fn create_manager(pool: &PgPool, name: &str, enabled: bool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO runner_managers (name, endpoint, access_token_hash, enabled) \
         VALUES ($1, $2, $3, $4) RETURNING runner_manager_id",
    )
    .bind(name)
    .bind(format!("http://{name}.invalid"))
    .bind(format!("hash-{}", Uuid::new_v4().simple()))
    .bind(enabled)
    .fetch_one(pool)
    .await
    .expect("insert runner manager")
}

/// Ensure one execution environment owned by `application_id`.
pub async fn create_environment(pool: &PgPool, application_id: i64, identity: &str) -> Uuid {
    match ensure_execution_environment(pool, application_id, identity, "opencode")
        .await
        .expect("ensure execution environment")
    {
        EnsureOutcome::Created(row) | EnsureOutcome::Existing(row) => row.environment_id,
        other => panic!("expected an environment row, got {other:?}"),
    }
}

/// Count every recorded operation row for one environment.
pub async fn count_jobs(pool: &PgPool, environment_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM execution_environment_jobs WHERE environment_id = $1")
        .bind(environment_id)
        .fetch_one(pool)
        .await
        .expect("count jobs")
}

/// Enqueue one ensure job and return its job id.
pub async fn seed_ensure(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    manager_id: i64,
    token: &str,
    deadline: DateTime<Utc>,
) -> Uuid {
    match enqueue_execution_environment_job(
        pool,
        application_id,
        environment_id,
        manager_id,
        "ensure",
        0,
        token,
        deadline,
    )
    .await
    .expect("enqueue ensure")
    {
        EnqueueOutcome::Created(row) => row.job_id,
        other => panic!("expected Created, got {other:?}"),
    }
}

/// A deadline comfortably in the future.
pub fn future_deadline() -> DateTime<Utc> {
    Utc::now() + ChronoDuration::seconds(600)
}

/// A deadline that has already passed.
pub fn past_deadline() -> DateTime<Utc> {
    Utc::now() - ChronoDuration::seconds(5)
}
