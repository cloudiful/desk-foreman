//! Unit tests for the environment-job store (no live database).
//!
//! These pin the pure pre-flight contracts that run before any query:
//! kind/state/token validation and terminal-state enforcement. Database
//! concurrency, tenant fencing, claim/complete races and expiry are covered
//! by `tests/execution_environment_jobs.rs` against an isolated nonproduction
//! PostgreSQL.

use chrono::Utc;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::lifecycle::complete_execution_environment_job;
use super::store::enqueue_execution_environment_job;
use super::types::ExecutionEnvironmentJobResult;

/// A pool that never connects: every pre-flight rejection must happen before a
/// query is attempted.
fn lazy_pool() -> PgPool {
    PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .expect("lazy pool builds without connecting")
}

#[tokio::test]
async fn enqueue_rejects_invalid_intent_before_any_query() {
    let pool = lazy_pool();
    let environment_id = Uuid::new_v4();
    let deadline = Utc::now() + chrono::Duration::seconds(600);

    assert!(
        enqueue_execution_environment_job(
            &pool,
            1,
            environment_id,
            1,
            "restart",
            0,
            "token",
            deadline,
        )
        .await
        .is_err()
    );

    assert!(
        enqueue_execution_environment_job(
            &pool,
            1,
            environment_id,
            1,
            "ensure",
            -1,
            "token",
            deadline,
        )
        .await
        .is_err()
    );

    assert!(
        enqueue_execution_environment_job(
            &pool,
            1,
            environment_id,
            1,
            "ensure",
            0,
            "   ",
            deadline,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn complete_rejects_non_terminal_state_before_any_query() {
    let pool = lazy_pool();
    assert!(
        complete_execution_environment_job(
            &pool,
            1,
            Uuid::new_v4(),
            "token",
            Uuid::new_v4(),
            1,
            "pending",
            Some(ExecutionEnvironmentJobResult::stop_confirmed("exited")),
            None,
        )
        .await
        .is_err()
    );
}
