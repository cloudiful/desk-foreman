//! Isolated database support for the environment-job integration tests.
//!
//! Reuses the shared nonproduction URL and application/token helper from the
//! execution-environment harness. A dedicated `JobDb` is needed on top of that
//! harness because the reconnect case must open a *second* pool against the
//! same database, and the frozen helper does not expose its URL.

use std::time::Duration;

use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::helpers::test_database_url;

fn replace_database_name(base_url: &str, db_name: &str) -> String {
    let (prefix, suffix) = match base_url.rfind('?') {
        Some(idx) => (&base_url[..idx], Some(&base_url[idx..])),
        None => (base_url, None),
    };
    let slash = prefix.rfind('/').expect("database URL must contain '/'");
    let head = &prefix[..slash + 1];
    match suffix {
        Some(query) => format!("{head}{db_name}{query}"),
        None => format!("{head}{db_name}"),
    }
}

/// Isolated per-test database with async cleanup and a reconnectable URL.
pub struct JobDb {
    pub pool: PgPool,
    pub url: String,
    admin_pool: PgPool,
    db_name: String,
}

impl JobDb {
    pub async fn setup() -> Self {
        let base_url = test_database_url();
        let admin_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&base_url)
            .await
            .expect("connect to base test database");
        let db_name = format!("df_job_test_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(&admin_pool)
        .await
        .expect("create isolated job test database");
        let url = replace_database_name(&base_url, &db_name);
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&url)
            .await
            .expect("connect to isolated job test database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations on isolated job test database");
        Self {
            pool,
            url,
            admin_pool,
            db_name,
        }
    }

    /// Open a fresh pool against the same database to prove durable state
    /// survives a reconnect rather than living in one connection.
    pub async fn reconnect(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&self.url)
            .await
            .expect("reconnect to isolated job test database")
    }

    pub async fn teardown(self) {
        let db_name = self.db_name.clone();
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(&self.admin_pool)
        .await
        .expect("drop isolated job test database");
        self.admin_pool.close().await;
    }
}
