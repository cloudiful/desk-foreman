//! Integration coverage for durable environment-operation records (issue 741 P1).
//!
//! Enqueue/tenancy fencing lives in `enqueue_cases`, manager validation and
//! active-operation races in `manager_cases`, claim/completion in
//! `claim_cases`, and expiry/reconnect in `expiry_cases`. Every case runs
//! against an isolated nonproduction PostgreSQL created from
//! `DF_ENV_TEST_DATABASE_URL`, reusing the execution-environment harness
//! through `#[path]`.

#[path = "execution_environments/helpers.rs"]
mod helpers;

#[path = "execution_environment_jobs/support.rs"]
mod support;

#[path = "execution_environment_jobs/fixtures.rs"]
mod fixtures;

#[path = "execution_environment_jobs/enqueue_cases.rs"]
mod enqueue_cases;

#[path = "execution_environment_jobs/manager_cases.rs"]
mod manager_cases;

#[path = "execution_environment_jobs/claim_cases.rs"]
mod claim_cases;

#[path = "execution_environment_jobs/expiry_cases.rs"]
mod expiry_cases;

#[path = "execution_environment_jobs/reviewer_cases.rs"]
mod reviewer_cases;
