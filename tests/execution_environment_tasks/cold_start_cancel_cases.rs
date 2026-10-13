//! Real cancellation of an in-flight cold native start (issue 824 P1r).
//!
//! The sibling case `a_failed_cold_environment_update_rolls_everything_back`
//! forces the environment UPDATE to fail with a SQL error. This one instead
//! *cancels* a real start that has already written the job and moved the task:
//! a private trigger parks the transaction on a gate an independent connection
//! holds, the caller aborts the handler future it owns, and the rolled-back
//! state is then read over a different pool.
//!
//! Nothing here goes through a live socket: the exported start service future
//! is driven directly in a task the caller owns, so aborting the join handle
//! really does cancel the whole handler instead of a client that walked away
//! from a still-running server.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use desk_foreman::db::execution_environments::{TaskExecutionRequest, start_native_task};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, Connection, Executor, PgConnection};
use uuid::Uuid;

use super::native_execution_helpers::schedule_preparation_job;
use super::support::TaskFixture;

/// The private advisory lock key, matching the gate trigger.
const GATE_KEY: i64 = 824_000_001;
/// Marks the backend that runs the start under test.
const START_APPLICATION_NAME: &str = "df-cold-start-cancel";
/// Upper bound on waiting for a real, positive condition.
const WAIT_BUDGET: Duration = Duration::from_secs(30);

fn read_sql(relative: &str) -> String {
    let path = format!(
        "{}/sql/tests/execution_environment_tasks/cold_start/{relative}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn start_path(fixture: &TaskFixture, task_id: Uuid) -> String {
    format!("{}/start", fixture.task_path(task_id))
}

/// The exact request the public handler forwards to the exported start service.
fn start_request(operation_token: &str) -> TaskExecutionRequest {
    TaskExecutionRequest {
        expected_generation: 4,
        operation_token: operation_token.to_string(),
    }
}

/// A pool whose every connection identifies itself, so the parked backend can
/// be located by name instead of guessed from timing.
async fn identifiable_pool(database: &sqlx::PgPool) -> sqlx::PgPool {
    let url = database.connect_options().to_url_lossy().to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _meta| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('application_name', $1, false)")
                    .bind(START_APPLICATION_NAME)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect the identifiable pool")
}

async fn blocked_backend_count(database: &sqlx::PgPool) -> i64 {
    let query = sqlx::AssertSqlSafe(read_sql("blocked_backend.sql"));
    let (count,): (i64,) = sqlx::query_as(query)
        .bind(START_APPLICATION_NAME)
        .fetch_one(database)
        .await
        .expect("observe blocked backends");
    count
}

async fn busy_backend_count(database: &sqlx::PgPool) -> i64 {
    let query = sqlx::AssertSqlSafe(read_sql("quiesced_backend.sql"));
    let (count,): (i64,) = sqlx::query_as(query)
        .bind(START_APPLICATION_NAME)
        .fetch_one(database)
        .await
        .expect("observe backend quiescence");
    count
}

/// Poll until the condition holds, yielding between probes.
///
/// The wait is bounded; it never sleeps for a fixed period and never turns a
/// missing condition into a pass.
async fn await_positive(
    label: &str,
    mut probe: impl FnMut() -> Pin<Box<dyn Future<Output = bool> + Send>>,
) {
    let deadline = Instant::now() + WAIT_BUDGET;
    loop {
        if probe().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {label}; the required condition never became true"
        );
        tokio::task::yield_now().await;
    }
}

async fn environment_row(
    fixture: &TaskFixture,
) -> (String, Option<i64>, Option<i64>, Option<String>) {
    sqlx::query_as(
        "SELECT state, assigned_manager_id, container_generation, active_native_task_id::text \
         FROM execution_environments WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("read the environment row")
}

async fn task_row(
    fixture: &TaskFixture,
    task_id: Uuid,
) -> (String, Option<i64>, Option<i64>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT state, assigned_manager_id, container_generation, start_job_id \
         FROM execution_environment_tasks WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("read the task row")
}

async fn start_job_count(fixture: &TaskFixture) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_jobs WHERE job_kind = $1",
    )
    .bind("start_native_task")
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count start jobs");
    count
}

#[tokio::test]
async fn a_cancelled_cold_start_rolls_back_the_job_task_and_binding() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();

    // The real public path up to a completed preparation. No helper wrote a
    // manager onto the environment.
    let (prepare_job_id, job) = schedule_preparation_job(&fixture, task_id).await;
    let payload: Value = job["payload"].clone();
    let ack = fixture
        .manager_result(
            prepare_job_id,
            true,
            json!({
                "task_id": task_id,
                "manifest_sha256": payload["manifest_sha256"],
                "file_count": payload["file_count"],
                "total_bytes": payload["total_bytes"],
                "prepared": true
            }),
        )
        .await;
    assert_eq!(ack.status, 204, "{}", ack.body_text());
    let (state, manager, candidate, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "still cold before the start");
    assert_eq!(candidate, None);

    // Private gate objects, created only in this test's own database.
    let gate_sql = sqlx::AssertSqlSafe(read_sql("cancellation_gate.sql"));
    fixture
        .database
        .pool
        .execute(gate_sql)
        .await
        .expect("install the private cancellation gate");

    // An independent connection holds the gate for the whole proof.
    let mut gate = PgConnection::connect_with(&fixture.database.pool.connect_options())
        .await
        .expect("connect the gate holder");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(GATE_KEY)
        .execute(&mut gate)
        .await
        .expect("hold the private gate");

    // The start runs on its own identifiable pool and through the exported
    // service the public handler awaits inline, so the caller owns the whole
    // future that owns the transaction.
    let start_pool = identifiable_pool(&fixture.database.pool).await;
    let application_id = fixture.application_id;
    let environment_id = fixture.environment_id;
    let start = tokio::spawn(async move {
        start_native_task(
            &start_pool,
            application_id,
            environment_id,
            task_id,
            start_request("cancel-cold-start"),
        )
        .await
    });

    let database = fixture.database.pool.clone();
    let mut probe = || {
        let database = database.clone();
        Box::pin(async move { blocked_backend_count(&database).await > 0 })
            as Pin<Box<dyn Future<Output = bool> + Send>>
    };
    // Positive evidence, not a timeout: this backend is provably waiting,
    // ungranted, on this advisory lock in this private database.
    await_positive(
        "the start transaction to block on the private gate",
        &mut probe,
    )
    .await;
    assert!(
        !start.is_finished(),
        "the start must still be parked on the gate"
    );

    start.abort();
    let cancelled = start
        .await
        .expect_err("the aborted start must not complete");
    assert!(cancelled.is_cancelled(), "the handler must be cancelled");

    // The gate is released only while the handler is already cancelled.
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(GATE_KEY)
        .execute(&mut gate)
        .await
        .expect("release the private gate");

    let mut probe = || {
        let database = database.clone();
        Box::pin(async move { busy_backend_count(&database).await == 0 })
            as Pin<Box<dyn Future<Output = bool> + Send>>
    };
    await_positive("the aborted backend to become quiescent", &mut probe).await;

    // Independent reads, over a pool the aborted handler never touched.
    let (state, manager, candidate, active) = environment_row(&fixture).await;
    assert_eq!(state, "absent", "the environment must not be starting");
    assert_eq!(manager, None, "the manager binding must be rolled back");
    assert_eq!(candidate, None);
    assert_eq!(active, Some(task_id.to_string()));
    let (task_state, pinned, task_candidate, start_job) = task_row(&fixture, task_id).await;
    assert_eq!(
        task_state, "prepared",
        "the task transition must be rolled back"
    );
    assert_eq!(start_job, None);
    assert_eq!(task_candidate, None);
    assert_eq!(
        pinned,
        Some(fixture.manager_id),
        "the task keeps its own pin"
    );
    assert_eq!(start_job_count(&fixture).await, 0);

    // Nothing about the cancelled attempt blocks a fresh start: the same
    // token is free again, because the first attempt never committed.
    let retried = fixture
        .json_post(
            &fixture.application_token,
            &start_path(&fixture, task_id),
            4,
            json!({"expected_generation": 4, "operation_token": "cancel-cold-start"}),
        )
        .await;
    assert_eq!(retried.status, 200, "{}", retried.body_text());
    let (state, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(state, "starting");
    assert_eq!(manager, Some(fixture.manager_id));
    assert_eq!(start_job_count(&fixture).await, 1);

    fixture.teardown().await;
}
