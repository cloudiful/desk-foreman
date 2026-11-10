//! Automatic DB-backed capability signing key (issue 1180 P39).
//!
//! The key is generated once with the platform CSPRNG and persisted in the
//! `git_gateway_signing_key` singleton row. Concurrent instances race on one
//! `INSERT ... ON CONFLICT DO NOTHING` and then read the persisted winner, so
//! every process signs with the same key and a restart preserves it. The key
//! material never leaves this module or `AppState`; callers receive only the
//! opaque HMAC key.

use anyhow::bail;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::PgPool;

pub const SIGNING_KEY_LEN: usize = 32;

/// Build an HMAC key from exactly 32 bytes. Used by startup after the
/// persisted row is read and by test fixtures that need a deterministic key.
pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<hmac::Key> {
    if bytes.len() != SIGNING_KEY_LEN {
        bail!("Git gateway signing key must be exactly {SIGNING_KEY_LEN} bytes");
    }
    Ok(hmac::Key::new(hmac::HMAC_SHA256, bytes))
}

/// Return the persisted signing key, creating it on first startup.
///
/// The insert is newline-terminated static SQL so the race between concurrent
/// instances resolves to the single persisted row instead of two keys.
#[derive(sqlx::FromRow)]
struct StoredSigningKey {
    signing_key: Vec<u8>,
}

pub async fn load_or_create(pool: &PgPool) -> anyhow::Result<hmac::Key> {
    let mut material = [0u8; SIGNING_KEY_LEN];
    SystemRandom::new()
        .fill(&mut material)
        .map_err(|_| anyhow::anyhow!("failed to generate Git gateway signing key"))?;
    sqlx::query_file!(
        "src/sql/git_gateway_signing_key/insert.sql",
        material.as_slice()
    )
    .execute(pool)
    .await?;
    let stored = sqlx::query_file_as!(StoredSigningKey, "src/sql/git_gateway_signing_key/get.sql")
        .fetch_one(pool)
        .await?;
    from_bytes(&stored.signing_key)
        .map_err(|_| anyhow::anyhow!("stored Git gateway signing key is invalid"))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sqlx::postgres::PgConnectOptions;

    use super::*;

    const REQUIRED_HOST: &str = "unraid.cloud1ful.com";
    const REQUIRED_PORT: u16 = 5444;
    const REQUIRED_DATABASE: &str = "desk_foreman_issue_1180";
    const REQUIRED_OWNERSHIP_MARKERS: [&str; 2] = ["issue 1180", "P39"];

    #[derive(sqlx::FromRow)]
    struct TestDatabaseIdentity {
        database_name: Option<String>,
        ownership_comment: Option<String>,
    }

    fn validate_test_endpoint(host: &str, port: u16, database: &str) -> anyhow::Result<()> {
        if host != REQUIRED_HOST || port != REQUIRED_PORT || database != REQUIRED_DATABASE {
            bail!("signing-key test refuses non-issue database target {host}:{port}/{database}");
        }
        Ok(())
    }

    fn validate_test_identity(identity: &TestDatabaseIdentity) -> anyhow::Result<()> {
        if identity.database_name.as_deref() != Some(REQUIRED_DATABASE) {
            bail!(
                "signing-key test refuses database {:?}",
                identity.database_name
            );
        }
        let comment = identity.ownership_comment.as_deref().unwrap_or_default();
        if !REQUIRED_OWNERSHIP_MARKERS
            .iter()
            .all(|marker| comment.contains(marker))
        {
            bail!(
                "signing-key test refuses database without the recorded issue 1180 P39 ownership comment"
            );
        }
        Ok(())
    }

    /// Connect only to the exact issue-owned test database after proving both
    /// the URL endpoint and the live database identity/ownership. Runs before
    /// any cleanup DELETE.
    async fn guarded_issue_pool() -> PgPool {
        let url = std::env::var("P29A_TEST_DATABASE_URL")
            .expect("P29A_TEST_DATABASE_URL must point at the issue-owned test database");
        let options = PgConnectOptions::from_str(&url)
            .unwrap_or_else(|_| panic!("P29A_TEST_DATABASE_URL is not a valid PostgreSQL URL"));
        validate_test_endpoint(
            options.get_host(),
            options.get_port(),
            options.get_database().unwrap_or_default(),
        )
        .expect("issue test database endpoint guard");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .expect("connect to the issue-owned signing-key test database");
        let identity = sqlx::query_file_as!(
            TestDatabaseIdentity,
            "src/sql/git_gateway_signing_key/test_guard.sql"
        )
        .fetch_one(&pool)
        .await
        .expect("read signing-key test database identity");
        validate_test_identity(&identity).expect("issue test database identity guard");
        pool
    }

    #[test]
    fn from_bytes_requires_exactly_32_bytes() {
        assert!(from_bytes(&[1u8; 32]).is_ok());
        assert!(from_bytes(&[1u8; 31]).is_err());
        assert!(from_bytes(&[1u8; 33]).is_err());
        assert!(from_bytes(&[]).is_err());
    }

    #[test]
    fn guard_rejects_wrong_endpoint_and_missing_ownership() {
        assert!(validate_test_endpoint(REQUIRED_HOST, REQUIRED_PORT, REQUIRED_DATABASE).is_ok());
        assert!(validate_test_endpoint("localhost", REQUIRED_PORT, REQUIRED_DATABASE).is_err());
        assert!(validate_test_endpoint(REQUIRED_HOST, 5432, REQUIRED_DATABASE).is_err());
        assert!(validate_test_endpoint(REQUIRED_HOST, REQUIRED_PORT, "postgres").is_err());

        let valid = TestDatabaseIdentity {
            database_name: Some(REQUIRED_DATABASE.to_string()),
            ownership_comment: Some(
                "issue 1180 P39 owned test DB; retain for issue review".to_string(),
            ),
        };
        assert!(validate_test_identity(&valid).is_ok());
        assert!(
            validate_test_identity(&TestDatabaseIdentity {
                database_name: Some("postgres".to_string()),
                ownership_comment: valid.ownership_comment.clone(),
            })
            .is_err()
        );
        assert!(
            validate_test_identity(&TestDatabaseIdentity {
                database_name: None,
                ownership_comment: valid.ownership_comment.clone(),
            })
            .is_err()
        );
        assert!(
            validate_test_identity(&TestDatabaseIdentity {
                database_name: Some(REQUIRED_DATABASE.to_string()),
                ownership_comment: Some("issue 1180 only".to_string()),
            })
            .is_err()
        );
        assert!(
            validate_test_identity(&TestDatabaseIdentity {
                database_name: Some(REQUIRED_DATABASE.to_string()),
                ownership_comment: None,
            })
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires the exact issue-owned PostgreSQL (P29A_TEST_DATABASE_URL)"]
    async fn concurrent_startup_persists_one_stable_key() -> anyhow::Result<()> {
        let pool = guarded_issue_pool().await;
        sqlx::query_file!("src/sql/git_gateway_signing_key/delete.sql")
            .execute(&pool)
            .await?;

        let (first, second) = tokio::join!(load_or_create(&pool), load_or_create(&pool));
        let first = first?;
        let second = second?;

        let stored =
            sqlx::query_file_as!(StoredSigningKey, "src/sql/git_gateway_signing_key/get.sql")
                .fetch_one(&pool)
                .await?;
        assert_eq!(stored.signing_key.len(), SIGNING_KEY_LEN);

        let message = b"capability";
        assert_eq!(
            hmac::sign(&first, message).as_ref(),
            hmac::sign(&second, message).as_ref()
        );
        let restarted = load_or_create(&pool).await?;
        assert_eq!(
            hmac::sign(&first, message).as_ref(),
            hmac::sign(&restarted, message).as_ref()
        );

        sqlx::query_file!("src/sql/git_gateway_signing_key/delete.sql")
            .execute(&pool)
            .await?;
        Ok(())
    }
}
