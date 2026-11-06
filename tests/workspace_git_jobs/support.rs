//! Guarded connection support for the durable Git-job suite.
//!
//! The suite never creates or drops a database. It connects only to the
//! task-owned issue database named in `DF_GIT_TEST_DATABASE_URL` after
//! proving the exact endpoint, database name, server version, and ownership
//! mark. A missing or mismatched environment fails loudly; it never becomes a
//! silently skipped pass.

use std::str::FromStr;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Serialize the suite on top of the database command's `--test-threads=1`.
pub static SUITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const REQUIRED_HOST: &str = "unraid.cloud1ful.com";
const REQUIRED_PORT: u16 = 5444;
const REQUIRED_DATABASE: &str = "desk_foreman_issue_1180";
const OWNERSHIP_MARK: &str = "desk-foreman issue #1180";

const IDENTITY_SQL: &str = include_str!("issue_db_identity.sql");

#[derive(sqlx::FromRow)]
struct IssueDbIdentity {
    database_name: String,
    server_version: String,
    ownership_comment: Option<String>,
}

pub struct IssueDb {
    pub pool: PgPool,
}

impl IssueDb {
    /// Validate the DSN before any connection, then connect to the one
    /// retained, already-initialized issue database.
    pub async fn setup() -> Self {
        let url = std::env::var("DF_GIT_TEST_DATABASE_URL").expect(
            "DF_GIT_TEST_DATABASE_URL is required for the workspace Git-job suite; set it to \
             the task-owned issue database URL (never production or a shared database)",
        );
        let options = PgConnectOptions::from_str(&url)
            .unwrap_or_else(|_| panic!("DF_GIT_TEST_DATABASE_URL is not a valid PostgreSQL URL"));
        assert_eq!(
            options.get_host(),
            REQUIRED_HOST,
            "DF_GIT_TEST_DATABASE_URL must target the confirmed test instance"
        );
        assert_eq!(
            options.get_port(),
            REQUIRED_PORT,
            "DF_GIT_TEST_DATABASE_URL must use the confirmed external test port"
        );
        assert_eq!(
            options.get_database(),
            Some(REQUIRED_DATABASE),
            "DF_GIT_TEST_DATABASE_URL must name the task-owned issue database"
        );

        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect_with(options)
            .await
            .expect("connect to the task-owned issue database");

        let identity: IssueDbIdentity = sqlx::query_as(IDENTITY_SQL)
            .fetch_one(&pool)
            .await
            .expect("read issue database identity");
        assert_eq!(
            identity.database_name, REQUIRED_DATABASE,
            "current_database() must be the task-owned issue database"
        );
        assert!(
            identity.server_version.starts_with("18."),
            "the issue database must be PostgreSQL 18.x, got {}",
            identity.server_version
        );
        assert!(
            identity
                .ownership_comment
                .as_deref()
                .is_some_and(|comment| comment.contains(OWNERSHIP_MARK)),
            "the issue database ownership comment must contain {OWNERSHIP_MARK:?}"
        );

        Self { pool }
    }
}
