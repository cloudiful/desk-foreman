pub mod applications;
pub mod approval;
pub mod audit;
pub mod queries;
pub mod sessions;
pub mod types;
pub mod users;
pub mod workspace_bindings;
pub mod workspace_runners;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;

use crate::{auth, config::AppConfig, workspace::default_user_workspace};

pub async fn connect(config: &AppConfig) -> anyhow::Result<sqlx::PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("failed to connect to postgres")
}

pub async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("failed to run database migrations")
}

pub async fn bootstrap_admin(pool: &sqlx::PgPool, config: &AppConfig) -> anyhow::Result<()> {
    let Some(login_name) = config.bootstrap_admin_login.as_deref() else {
        return Ok(());
    };
    let Some(password) = config.bootstrap_admin_password.as_deref() else {
        return Ok(());
    };
    let admin_count = queries::count_admin_users(pool).await?;
    if admin_count > 0 {
        return Ok(());
    }

    let display_name = config
        .bootstrap_admin_display_name
        .clone()
        .unwrap_or_else(|| "Administrator".to_string());
    let email = config
        .bootstrap_admin_email
        .clone()
        .unwrap_or_else(|| format!("{login_name}@local.invalid"));
    let password_hash = auth::hash_password(password)?;
    queries::bootstrap_admin(
        pool,
        login_name,
        &password_hash,
        &display_name,
        &email,
        &config.bootstrap_admin_timezone,
    )
    .await?;

    if let Some(user) = queries::find_user_by_login(pool, login_name).await?
        && user.workspace_root.as_deref().unwrap_or("").is_empty()
    {
        let workspace_root = default_user_workspace(&config.workspace_root, user.user_id)
            .to_string_lossy()
            .to_string();
        let _ = queries::update_user_workspace(pool, user.user_id, &workspace_root).await?;
    }

    Ok(())
}
