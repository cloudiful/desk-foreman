//! Support for the runner-manager heartbeat throttle cases.
//!
//! Every case drives the real internal HTTP router against an isolated
//! nonproduction PostgreSQL database created from `DF_ENV_TEST_DATABASE_URL`,
//! reusing the shared execution-environment harness. Timestamps are seeded
//! directly so no case sleeps through a ten-second window. Every static
//! fixture statement lives in `sql/` and is loaded with `include_str!`; the
//! production statements are loaded through the same pattern so the boundary
//! cases exercise the shipped predicates directly.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgPool, Postgres};
use tokio::task::JoinHandle;

use desk_foreman::db::{
    queries,
    types::{CreateRunnerManagerRequest, RunnerManagerAuthRecord, RunnerManagerRecord},
};

use super::{
    helpers::{IsolatedDb, test_state},
    http::{http_get, spawn_http},
};

pub const TOKEN: &str = "dfm_heartbeat_1176_token";

pub const FIND_AUTH_SQL: &str = include_str!("../../src/sql/find_runner_manager_by_token.sql");
pub const TOUCH_SQL: &str = include_str!("../../src/sql/touch_runner_manager.sql");
pub const HEARTBEAT_WRITE_INTERVAL_SECS: i64 =
    runner_protocol::RUNNER_MANAGER_HEARTBEAT_WRITE_INTERVAL_SECS as i64;

const SEED_SQL: &str = include_str!("sql/seed_runner_manager.sql");
const SELECT_ROW_SQL: &str = include_str!("sql/select_runner_manager.sql");
const DATABASE_NOW_SQL: &str = include_str!("sql/database_now.sql");
const LOCK_ROW_SQL: &str = include_str!("sql/lock_runner_manager.sql");
const TX_BEGIN_SQL: &str = include_str!("sql/transaction_begin.sql");
const TX_ROLLBACK_SQL: &str = include_str!("sql/transaction_rollback.sql");
const CREATE_TOUCH_LOG_SQL: &str = include_str!("sql/create_touch_log.sql");
const CLEAR_TOUCH_LOG_SQL: &str = include_str!("sql/clear_touch_log.sql");
const COUNT_TOUCHES_SQL: &str = include_str!("sql/count_touches.sql");
const CREATE_TOUCH_FAILURE_SQL: &str = include_str!("sql/create_touch_failure.sql");
const SET_TOUCH_FAILURE_SQL: &str = include_str!("sql/set_touch_failure.sql");
const BLOCKED_UPDATE_PIDS_SQL: &str = include_str!("sql/blocked_update_pids.sql");
const CANCEL_BACKEND_SQL: &str = include_str!("sql/cancel_backend.sql");

pub struct ManagerRow {
    pub status: String,
    pub enabled: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

type RowTuple = (String, bool, Option<DateTime<Utc>>, DateTime<Utc>);

fn manager_row((status, enabled, last_seen_at, updated_at): RowTuple) -> ManagerRow {
    ManagerRow {
        status,
        enabled,
        last_seen_at,
        updated_at,
    }
}

pub async fn begin_transaction(conn: &mut PgConnection) {
    sqlx::query(TX_BEGIN_SQL)
        .execute(conn)
        .await
        .expect("begin transaction");
}

pub async fn rollback_transaction(conn: &mut PgConnection) {
    sqlx::query(TX_ROLLBACK_SQL)
        .execute(conn)
        .await
        .expect("rollback transaction");
}

pub async fn database_now(conn: &mut PgConnection) -> DateTime<Utc> {
    sqlx::query_scalar(DATABASE_NOW_SQL)
        .fetch_one(conn)
        .await
        .expect("database clock")
}

pub async fn seed_row(
    conn: &mut PgConnection,
    manager_id: i64,
    enabled: bool,
    status: &str,
    last_seen_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
) -> ManagerRow {
    sqlx::query_as::<_, RowTuple>(SEED_SQL)
        .bind(manager_id)
        .bind(enabled)
        .bind(status)
        .bind(last_seen_at)
        .bind(updated_at)
        .fetch_one(conn)
        .await
        .map(manager_row)
        .expect("seed runner manager heartbeat")
}

pub async fn select_row(conn: &mut PgConnection, manager_id: i64) -> ManagerRow {
    sqlx::query_as::<_, RowTuple>(SELECT_ROW_SQL)
        .bind(manager_id)
        .fetch_one(conn)
        .await
        .map(manager_row)
        .expect("read runner manager row")
}

pub async fn blocked_update_pids(conn: &mut PgConnection) -> Vec<i32> {
    sqlx::query(include_str!("sql/clear_stats_snapshot.sql"))
        .execute(&mut *conn)
        .await
        .expect("clear cached activity snapshot");
    sqlx::query_scalar(BLOCKED_UPDATE_PIDS_SQL)
        .fetch_all(conn)
        .await
        .expect("read blocked update backends")
}

pub async fn wait_for_blocked_updates(conn: &mut PgConnection, expected: usize) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pids = blocked_update_pids(conn).await;
        if pids.len() >= expected {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "only {} of {expected} guarded UPDATEs reached the held row lock",
            pids.len()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn wait_for_no_blocked_updates(conn: &mut PgConnection) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if blocked_update_pids(conn).await.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "a cancelled guarded UPDATE remained blocked on the held row lock"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn cancel_backend(conn: &mut PgConnection, pid: i32) -> bool {
    sqlx::query_scalar(CANCEL_BACKEND_SQL)
        .bind(pid)
        .fetch_one(conn)
        .await
        .expect("cancel pending backend")
}

pub struct HeartbeatFixture {
    pub database: IsolatedDb,
    pub base: String,
    pub manager_id: i64,
    handle: JoinHandle<()>,
}

impl HeartbeatFixture {
    pub async fn setup() -> Self {
        let database = IsolatedDb::setup().await;
        let state = test_state(database.pool.clone());
        let (base, handle) = spawn_http(state).await;
        let (record, _) = queries::create_runner_manager(
            &database.pool,
            &CreateRunnerManagerRequest {
                name: "heartbeat-1176".to_string(),
                endpoint: "http://heartbeat-1176.invalid".to_string(),
                access_token: Some(TOKEN.to_string()),
                image: "test-image".to_string(),
                network_enabled: false,
                max_output_bytes: 65_536,
                max_timeout_ms: 600_000,
                max_sessions: 2,
                pids_limit: 64,
                memory_limit: "256m".to_string(),
                cpu_limit: "1".to_string(),
                host_workspace_root: None,
            },
        )
        .await
        .expect("create runner manager");
        Self {
            database,
            base,
            manager_id: record.runner_manager_id,
            handle,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.database.pool
    }

    pub async fn config(&self) -> (u16, Value) {
        self.config_with_token(TOKEN).await
    }

    pub async fn config_with_token(&self, token: &str) -> (u16, Value) {
        http_get(
            &self.base,
            "/api/internal/runner-manager/config",
            Some(token),
        )
        .await
    }

    pub fn spawn_config_request(&self) -> JoinHandle<(u16, Value)> {
        let base = self.base.clone();
        tokio::spawn(async move {
            http_get(&base, "/api/internal/runner-manager/config", Some(TOKEN)).await
        })
    }

    pub async fn auth(&self) -> Option<RunnerManagerAuthRecord> {
        queries::find_runner_manager_auth_by_token(self.pool(), TOKEN)
            .await
            .expect("read runner manager auth row")
    }

    pub async fn record_by_token(&self) -> Option<RunnerManagerRecord> {
        queries::find_runner_manager_by_token(self.pool(), TOKEN)
            .await
            .expect("read runner manager record")
    }

    pub async fn seed(&self, enabled: bool, status: &str, age_seconds: Option<f64>) -> ManagerRow {
        let now = Utc::now();
        let last_seen_at =
            age_seconds.map(|secs| now - chrono::Duration::milliseconds((secs * 1_000.0) as i64));
        self.seed_at(enabled, status, last_seen_at, now).await
    }

    pub async fn seed_at(
        &self,
        enabled: bool,
        status: &str,
        last_seen_at: Option<DateTime<Utc>>,
        updated_at: DateTime<Utc>,
    ) -> ManagerRow {
        let mut conn = self
            .pool()
            .acquire()
            .await
            .expect("acquire seed connection");
        seed_row(
            &mut conn,
            self.manager_id,
            enabled,
            status,
            last_seen_at,
            updated_at,
        )
        .await
    }

    pub async fn row(&self) -> ManagerRow {
        let mut conn = self
            .pool()
            .acquire()
            .await
            .expect("acquire read connection");
        select_row(&mut conn, self.manager_id).await
    }

    pub async fn install_touch_log(&self) {
        sqlx::raw_sql(CREATE_TOUCH_LOG_SQL)
            .execute(self.pool())
            .await
            .expect("install touch log");
    }

    pub async fn clear_touch_log(&self) {
        sqlx::query(CLEAR_TOUCH_LOG_SQL)
            .execute(self.pool())
            .await
            .expect("clear touch log");
    }

    pub async fn touch_count(&self) -> i64 {
        sqlx::query_scalar(COUNT_TOUCHES_SQL)
            .fetch_one(self.pool())
            .await
            .expect("count touches")
    }

    pub async fn install_touch_failure(&self) {
        sqlx::raw_sql(CREATE_TOUCH_FAILURE_SQL)
            .execute(self.pool())
            .await
            .expect("install touch failure");
    }

    pub async fn set_touch_failure(&self, fail: bool) {
        sqlx::query(SET_TOUCH_FAILURE_SQL)
            .bind(fail)
            .execute(self.pool())
            .await
            .expect("toggle touch failure");
    }

    pub async fn hold_row_lock(&self) -> sqlx::pool::PoolConnection<Postgres> {
        let mut conn = self
            .pool()
            .acquire()
            .await
            .expect("acquire lock connection");
        begin_transaction(&mut conn).await;
        sqlx::query(LOCK_ROW_SQL)
            .bind(self.manager_id)
            .execute(&mut *conn)
            .await
            .expect("lock runner manager row");
        conn
    }

    pub async fn release_row_lock(mut conn: sqlx::pool::PoolConnection<Postgres>) {
        rollback_transaction(&mut conn).await;
    }

    pub async fn teardown(self) {
        self.handle.abort();
        self.database.teardown().await;
    }
}
