//! Durable environment-operation store (issue 741 P1) and assigned-manager
//! dispatch (issue 742 P2).
//!
//! Thin module entry: focused types, checked persistence, pure result
//! evaluation, and unit tests live in the sibling directory so this file stays
//! wiring-only.

mod dispatch;
#[cfg(test)]
mod dispatch_tests;
mod lifecycle;
mod resolution;
#[cfg(test)]
mod resolution_tests;
mod store;
mod types;

#[cfg(test)]
mod tests;

pub use lifecycle::{
    ClaimOutcome, CompleteOutcome, claim_execution_environment_job,
    complete_execution_environment_job, expire_execution_environment_jobs,
};
pub use store::{
    EnqueueOutcome, enqueue_execution_environment_job, find_active_execution_environment_job,
    find_execution_environment_job_by_token,
};
pub use types::{
    EXECUTION_ENVIRONMENT_JOB_ACTIVE_STATES, EXECUTION_ENVIRONMENT_JOB_ERROR_SUMMARY_MAX_LEN,
    EXECUTION_ENVIRONMENT_JOB_KINDS, EXECUTION_ENVIRONMENT_JOB_STATES,
    EXECUTION_ENVIRONMENT_JOB_TERMINAL_STATES, EXECUTION_ENVIRONMENT_JOB_TOKEN_MAX_LEN,
    ExecutionEnvironmentJob, ExecutionEnvironmentJobResult, job_kind_is_valid, job_state_is_active,
    job_state_is_terminal, job_state_is_valid, job_token_is_valid, redact_error_summary,
};

// Manager-only durable dispatch helpers (issue 742 P2). Kept crate-internal:
// the durable lookup is not a public client store and is only reachable from
// the authenticated runner-manager result path.
pub(crate) use dispatch::{
    DurableJobCompletion, claim_next_assigned_manager_environment_job,
    complete_assigned_manager_environment_job, encode_environment_job_wire_id,
    is_environment_job_wire_id, parse_environment_job_wire_id,
};
pub(crate) use resolution::{environment_job_owner, environment_job_wire_kind};
