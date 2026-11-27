//! Application and bearer-token fixture for the execution-environment
//! integration suites.

use desk_foreman::db::types::{CreateApplicationRequest, CreateApplicationTokenRequest};
use sqlx::PgPool;

/// Create one application plus bearer token for auth tests.
pub async fn create_app_with_token(pool: &PgPool, name: &str) -> (i64, String) {
    let app = desk_foreman::db::queries::create_application(
        pool,
        &CreateApplicationRequest {
            name: name.to_string(),
            workspace_template: None,
            default_shell: None,
            default_scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await
    .expect("create application");
    let (token, _) = desk_foreman::db::queries::create_application_token(
        pool,
        &CreateApplicationTokenRequest {
            application_id: app.application_id,
            token_name: format!("{name}-token"),
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await
    .expect("create application token");
    (app.application_id, token)
}
