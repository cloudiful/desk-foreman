//! Admin-bootstrapped state and admin session helper (issue 753 P1).
//!
//! Split from `db_fixtures.rs` so each file stays inside the size budget. The
//! state is bootstrapped through the project's own helper and the session is
//! minted over the real login route, so a test never needs a live admin
//! server or a fabricated cookie.

use std::{path::PathBuf, sync::Arc};

use desk_foreman::AppState;
use sqlx::PgPool;

use super::support::call;

pub async fn test_state_with_admin(
    pool: PgPool,
    workspace_root: &str,
    login_name: &str,
    password: &str,
) -> AppState {
    let base = super::helpers::test_state(pool.clone());
    let mut config = (*base.config).clone();
    config.workspace_root = PathBuf::from(workspace_root);
    config.bootstrap_admin_login = Some(login_name.to_string());
    config.bootstrap_admin_password = Some(password.to_string());
    config.bootstrap_admin_display_name = Some("Native Task Admin".to_string());
    let state = AppState {
        config: Arc::new(config),
        runner: Arc::clone(&base.runner),
        runner_broker: Arc::clone(&base.runner_broker),
        db: pool,
    };
    desk_foreman::db::bootstrap_admin(&state.db, &state.config)
        .await
        .expect("bootstrap admin");
    // A bootstrapped admin is forced through a password change before it can
    // use any route. This fixture sets its own password, so the first-login
    // gate is satisfied and no change is outstanding.
    sqlx::query("UPDATE users SET must_change_password = FALSE WHERE login_name = $1")
        .bind(login_name)
        .execute(&state.db)
        .await
        .expect("clear first-login password gate");
    state
}

/// Log in over the real route and return the session cookie pair.
pub async fn admin_session(base: &str, login_name: &str, password: &str) -> String {
    let outcome = call(
        base,
        "POST",
        "/api/auth/login",
        None,
        &[],
        serde_json::to_vec(&serde_json::json!({
            "login_name": login_name,
            "password": password,
        }))
        .expect("login body")
        .as_slice(),
    )
    .await;
    assert_eq!(
        outcome.status,
        200,
        "admin login must succeed: {}",
        outcome.body_text()
    );
    // `call` reads the raw head, so the Set-Cookie header is available here.
    let cookie = outcome
        .set_cookie
        .as_deref()
        .expect("a successful login sets a session cookie");
    cookie
        .split(';')
        .next()
        .expect("cookie name=value")
        .to_string()
}
