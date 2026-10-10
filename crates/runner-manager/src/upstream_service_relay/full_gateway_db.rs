//! Isolated per-test database for the full-gateway proof (issue 749 P2d,
//! `#[cfg(test)]`).
//!
//! Split from [`super::full_gateway_support`] so no file exceeds the size
//! budget. Each test creates its own database from `DF_ENV_TEST_DATABASE_URL`
//! and drops it afterwards; the shared database is never reset or truncated.
//! A missing URL fails the run instead of silently passing.

#[cfg(test)]
pub(crate) mod db {
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use uuid::Uuid;

    /// Dedicated nonproduction test database URL. Panics when missing so
    /// the mandatory gate fails instead of silently passing.
    pub fn test_database_url() -> String {
        std::env::var("DF_ENV_TEST_DATABASE_URL").expect(
            "DF_ENV_TEST_DATABASE_URL is required for full-gateway tests; \
             set it to a dedicated nonproduction PostgreSQL URL (never production)",
        )
    }

    fn replace_database_name(base_url: &str, db_name: &str) -> String {
        let (prefix, suffix) = match base_url.rfind('?') {
            Some(idx) => (&base_url[..idx], Some(&base_url[idx..])),
            None => (base_url, None),
        };
        let slash = prefix.rfind('/').expect("database URL must contain '/'");
        let head = &prefix[..slash + 1];
        match suffix {
            Some(q) => format!("{head}{db_name}{q}"),
            None => format!("{head}{db_name}"),
        }
    }

    /// Isolated per-test database with async cleanup.
    pub struct IsolatedDb {
        pub pool: PgPool,
        admin_pool: PgPool,
        db_name: String,
    }

    impl IsolatedDb {
        pub async fn setup() -> Self {
            let base_url = test_database_url();
            let admin_pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&base_url)
                .await
                .expect("connect to base test database");
            let db_name = format!(
                "df_gateway_test_{}",
                Uuid::new_v4().to_string().replace('-', "")
            );
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE DATABASE \"{db_name}\""
            )))
            .execute(&admin_pool)
            .await
            .expect("create isolated test database");
            let db_url = replace_database_name(&base_url, &db_name);
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&db_url)
                .await
                .expect("connect to isolated test database");
            sqlx::migrate!("../../migrations")
                .run(&pool)
                .await
                .expect("run migrations on isolated test database");
            Self {
                pool,
                admin_pool,
                db_name,
            }
        }

        pub async fn teardown(self) {
            let db_name = self.db_name.clone();
            self.pool.close().await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
            )))
            .execute(&self.admin_pool)
            .await
            .expect("drop isolated test database");
            self.admin_pool.close().await;
        }
    }
}
