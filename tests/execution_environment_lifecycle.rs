//! Integration coverage for authenticated asynchronous execution environment
//! lifecycle (issue 742 P3).
//!
//! Drives a mock runner-manager through the real internal HTTP surface: the
//! authenticated provision/stop/replace/status routes plus the P2 job pull and
//! result transport. Every case runs against an isolated nonproduction
//! PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`; the execution
//! environment harness is reused through `#[path]` to avoid duplication.

#[path = "execution_environments/helpers.rs"]
mod helpers;

#[path = "execution_environments/http.rs"]
mod http;

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
