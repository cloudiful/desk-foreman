//! Integration coverage for durable assigned-manager environment-job
//! dispatch (issue 742 P2).
//!
//! Covers pull (claim) and result transport over the authenticated internal
//! runner-manager routes, the reserved `envjob:<UUID>` wire namespace, and
//! the unchanged legacy broker path. Every case runs against an isolated
//! nonproduction PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`; the
//! execution-environment harness is reused through `#[path]` to avoid
//! duplication.

#[path = "execution_environments/test_database_url.rs"]
mod test_database_url;

#[path = "execution_environments/app_state.rs"]
mod app_state;

#[path = "execution_environments/app_token.rs"]
mod app_token;

#[path = "execution_environments/http_server.rs"]
mod http_server;

#[path = "execution_environments/http_client.rs"]
mod http_client;

#[path = "execution_environments/http_get.rs"]
mod http_get;

#[path = "execution_environments/http_post.rs"]
mod http_post;

#[path = "execution_environment_dispatch/support.rs"]
mod support;

#[path = "execution_environment_dispatch/ack.rs"]
mod ack;

#[path = "execution_environment_dispatch/pull_cases.rs"]
mod pull_cases;

#[path = "execution_environment_dispatch/result_cases.rs"]
mod result_cases;

#[path = "execution_environment_dispatch/result_failure_cases.rs"]
mod result_failure_cases;

#[path = "execution_environment_dispatch/legacy_cases.rs"]
mod legacy_cases;
