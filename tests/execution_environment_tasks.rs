//! Native task integration coverage (issue 753 P1).
//!
//! Drives the real internal HTTP surface against an isolated nonproduction
//! PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`. What this suite
//! proves is the control plane's own exclusivity and lifecycle contract: who
//! may admit a task against which workspace, what blocks it, and what a
//! cancel or release may conclude.
//!
//! It deliberately proves nothing about Docker, containers, mounts,
//! checkouts or a real task process: P1 has no writer.

#[path = "execution_environments/helpers.rs"]
mod helpers;

#[path = "execution_environments/http.rs"]
mod http;

#[path = "execution_environment_tasks/support.rs"]
mod support;

#[path = "execution_environment_tasks/http_client.rs"]
mod http_client;

#[path = "execution_environment_tasks/db_fixtures.rs"]
mod db_fixtures;

#[path = "execution_environment_tasks/admin_state.rs"]
mod admin_state;

#[path = "execution_environment_tasks/task_seed.rs"]
mod task_seed;

#[path = "execution_environment_tasks/admission_cases.rs"]
mod admission_cases;

#[path = "execution_environment_tasks/candidate_cases.rs"]
mod candidate_cases;

#[path = "execution_environment_tasks/exclusivity_cases.rs"]
mod exclusivity_cases;

#[path = "execution_environment_tasks/cancel_cases.rs"]
mod cancel_cases;

#[path = "execution_environment_tasks/release_cases.rs"]
mod release_cases;

#[path = "execution_environment_tasks/terminal_fence_cases.rs"]
mod terminal_fence_cases;

#[path = "execution_environment_tasks/concurrency_cases.rs"]
mod concurrency_cases;

#[path = "execution_environment_tasks/seed_cases.rs"]
mod seed_cases;

#[path = "execution_environment_tasks/seed_access_cases.rs"]
mod seed_access_cases;

#[path = "execution_environment_tasks/seed_bounds_cases.rs"]
mod seed_bounds_cases;

#[path = "execution_environment_tasks/seed_concurrency_cases.rs"]
mod seed_concurrency_cases;

#[path = "execution_environment_tasks/prepare_helpers.rs"]
mod prepare_helpers;

#[path = "execution_environment_tasks/prepare_cases.rs"]
mod prepare_cases;

#[path = "execution_environment_tasks/prepare_access_cases.rs"]
mod prepare_access_cases;

#[path = "execution_environment_tasks/prepare_concurrency_cases.rs"]
mod prepare_concurrency_cases;

#[path = "execution_environment_tasks/prepare_fence_cases.rs"]
mod prepare_fence_cases;

#[path = "execution_environment_tasks/prepare_atomic_cases.rs"]
mod prepare_atomic_cases;

#[path = "execution_environment_tasks/prepare_race_cases.rs"]
mod prepare_race_cases;

#[path = "execution_environment_tasks/prepare_token_cases.rs"]
mod prepare_token_cases;

#[path = "execution_environment_tasks/native_execution_helpers.rs"]
mod native_execution_helpers;

#[path = "execution_environment_tasks/native_execution_cases.rs"]
mod native_execution_cases;

#[path = "execution_environment_tasks/reviewer_relay_cutoff_cases.rs"]
mod reviewer_relay_cutoff_cases;

#[path = "execution_environment_tasks/reviewer_native_historical_cases.rs"]
mod reviewer_native_historical_cases;

#[path = "execution_environment_tasks/reviewer_native_atomic_cases.rs"]
mod reviewer_native_atomic_cases;

#[path = "execution_environment_tasks/reviewer_native_replay_cases.rs"]
mod reviewer_native_replay_cases;

#[path = "execution_environment_tasks/native_job_constraint_cases.rs"]
mod native_job_constraint_cases;

#[path = "execution_environment_tasks/lease_helpers.rs"]
mod lease_helpers;

#[path = "execution_environment_tasks/legacy_guard_cases.rs"]
mod legacy_guard_cases;

#[path = "execution_environment_tasks/legacy_lease_cases.rs"]
mod legacy_lease_cases;

#[path = "execution_environment_tasks/admin_fixture.rs"]
mod admin_fixture;

#[path = "execution_environment_tasks/admin_guard_cases.rs"]
mod admin_guard_cases;

#[path = "execution_environment_tasks/janitor_cases.rs"]
mod janitor_cases;

#[path = "execution_environment_tasks/export_helpers.rs"]
mod export_helpers;

#[path = "execution_environment_tasks/export_cases.rs"]
mod export_cases;

#[path = "execution_environment_tasks/export_atomic_cases.rs"]
mod export_atomic_cases;

#[path = "execution_environment_tasks/export_access_cases.rs"]
mod export_access_cases;

#[path = "execution_environment_tasks/export_claim_cases.rs"]
mod export_claim_cases;

#[path = "execution_environment_tasks/export_bounds_cases.rs"]
mod export_bounds_cases;
