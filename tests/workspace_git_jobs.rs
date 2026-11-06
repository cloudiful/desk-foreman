//! Guarded integration coverage for durable Git-job storage (issue 1180 DFI3a1a).
//!
//! Every case runs against the task-owned `desk_foreman_issue_1180` database
//! supplied through `DF_GIT_TEST_DATABASE_URL`; the suite never creates or
//! drops a database. Admission cases cover locked capacity, guards cover every
//! existing workspace writer, and expiry cases cover the closed completion
//! state machine.

#[path = "workspace_git_jobs/support.rs"]
mod support;

#[path = "workspace_git_jobs/fixtures.rs"]
mod fixtures;

#[path = "workspace_git_jobs/admission_cases.rs"]
mod admission_cases;

#[path = "workspace_git_jobs/guard_cases.rs"]
mod guard_cases;

#[path = "workspace_git_jobs/expiry_cases.rs"]
mod expiry_cases;

#[path = "workspace_git_jobs/claim_cases.rs"]
mod claim_cases;

#[path = "workspace_git_jobs/auth_cases.rs"]
mod auth_cases;

#[path = "workspace_git_jobs/cutover_cases.rs"]
mod cutover_cases;

#[path = "workspace_git_jobs/reconcile_cases.rs"]
mod reconcile_cases;
