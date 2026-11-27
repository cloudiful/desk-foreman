//! Regression coverage for the runner-manager heartbeat persistence throttle
//! (issue 1176 P1).
//!
//! Every case drives the real internal HTTP authentication path against an
//! isolated nonproduction PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`;
//! the shared execution-environment harness supplies the isolated database,
//! router and HTTP client, so a missing prerequisite fails instead of skipping.

#[path = "execution_environments/test_database_url.rs"]
mod test_database_url;

#[path = "execution_environments/isolated_db.rs"]
mod isolated_db;

#[path = "execution_environments/app_state.rs"]
mod app_state;

#[path = "execution_environments/http_server.rs"]
mod http_server;

#[path = "execution_environments/http_client.rs"]
mod http_client;

#[path = "execution_environments/http_get.rs"]
mod http_get;

#[path = "runner_manager_heartbeat/support.rs"]
mod support;

#[path = "runner_manager_heartbeat/cases.rs"]
mod cases;
