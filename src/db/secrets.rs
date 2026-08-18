use anyhow::{Context, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::Rng;
use sqlx::PgPool;

const MASTER_SECRET_NAME: &str = "master_secret";

pub async fn get_or_create_master_secret(
    pool: &PgPool,
    seed: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(value) = get_master_secret(pool).await? {
        return Ok(value);
    }

    let generated = match seed.map(str::trim).filter(|value| !value.is_empty()) {
        Some(seed) => {
            let bytes = STANDARD
                .decode(seed.as_bytes())
                .context("DESK_FOREMAN_SECRET_MASTER_KEY must be valid base64")?;
            if bytes.len() != 32 {
                return Err(anyhow!(
                    "DESK_FOREMAN_SECRET_MASTER_KEY must decode to 32 bytes, got {}",
                    bytes.len()
                ));
            }
            seed.to_string()
        }
        None => {
            let mut bytes = [0_u8; 32];
            rand::rng().fill_bytes(&mut bytes);
            STANDARD.encode(bytes)
        }
    };
    let inserted = sqlx::query_scalar(include_str!("../sql/insert_app_secret.sql"))
        .bind(MASTER_SECRET_NAME)
        .bind(&generated)
        .fetch_optional(pool)
        .await
        .context("failed to persist generated application secret")?;
    if let Some(value) = inserted {
        return Ok(value);
    }

    let stored = get_master_secret(pool)
        .await?
        .ok_or_else(|| anyhow!("application secret disappeared after creation"))?;
    if seed
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| value != stored)
    {
        return Err(anyhow!(
            "configured application secret seed does not match the persisted secret"
        ));
    }
    Ok(stored)
}

async fn get_master_secret(pool: &PgPool) -> anyhow::Result<Option<String>> {
    sqlx::query_scalar(include_str!("../sql/get_app_secret.sql"))
        .bind(MASTER_SECRET_NAME)
        .fetch_optional(pool)
        .await
        .context("failed to load application secret")
}
