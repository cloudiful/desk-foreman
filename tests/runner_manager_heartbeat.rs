//! Regression coverage for the runner-manager heartbeat persistence throttle
//! (issue 1176 P1).
//!
//! Every case drives the real internal HTTP authentication path against an
//! isolated nonproduction PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`;
//! the shared execution-environment harness supplies the isolated database,
//! router and HTTP client, so a missing prerequisite fails instead of skipping.

#[path = "execution_environments/helpers.rs"]
mod helpers;

#[path = "execution_environments/http.rs"]
mod http;

#[path = "runner_manager_heartbeat/support.rs"]
mod support;

#[path = "runner_manager_heartbeat/cases.rs"]
mod cases;
