//! Integration coverage for authenticated asynchronous execution environment
//! lifecycle (issue 742 P3).
//!
//! Drives a mock runner-manager through the real internal HTTP surface: the
//! authenticated provision/stop/replace/status routes plus the P2 job pull and
//! result transport. Every case runs against an isolated nonproduction
//! PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`; the execution
//! environment harness is reused through `#[path]` to avoid duplication.

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

#[path = "execution_environment_lifecycle/support.rs"]
mod support;

#[path = "execution_environment_lifecycle/requests.rs"]
mod requests;

#[path = "execution_environment_lifecycle/loop_cases.rs"]
mod loop_cases;

#[path = "execution_environment_lifecycle/recovery_cases.rs"]
mod recovery_cases;

#[path = "execution_environment_lifecycle/fence_cases.rs"]
mod fence_cases;

#[path = "execution_environment_lifecycle/concurrency_cases.rs"]
mod concurrency_cases;

#[path = "execution_environment_lifecycle/reviewer_cases.rs"]
mod reviewer_cases;
