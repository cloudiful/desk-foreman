//! Authenticated asynchronous execution environment lifecycle (issue 742 P3).
//!
//! `provision`, `stop`, `replace` and `status` run as short owner-scoped
//! transactions: they lock the environment row `FOR UPDATE`, expire overdue
//! jobs, validate the caller's fences, reconcile persisted evidence, and admit
//! exactly one durable operation intent. No network or Docker work happens
//! while the row lock is held, and no operation is admitted without a durable
//! candidate and manager affinity.

mod provision;
mod replace;
mod status;
mod stop;
mod support;
mod transitions;
mod types;

pub use provision::provision_execution_environment;
pub use replace::replace_execution_environment;
pub use status::execution_environment_status;
pub use stop::stop_execution_environment;

// Visibility-only: the native-task phase reuses the same lock-held helpers so
// task admission applies identical blocker and manager-selection rules rather
// than a second copy that could drift.
pub(crate) use support::{expire_overdue_jobs, find_job_by_id, load_jobs, select_manager};

// Visibility-only: native-task admission reuses the exact confirmed-stop rule
// replacement already applies, so the two can never drift apart.
pub(crate) use replace::proof_is_confirmed_stop;

pub use types::{
    ACCEPTED_CONFIRMED_STOP_STATES, EnvironmentOperationDescriptor, LIFECYCLE_ENVIRONMENT_STATES,
    LIFECYCLE_STATE_READY, LIFECYCLE_STATE_REPLACED, LIFECYCLE_STATE_STARTING,
    LIFECYCLE_STATE_STOPPING, LIFECYCLE_STATE_UNCERTAIN, LifecycleEnvironment, OperationDescriptor,
    SUPERSEDED_BY_STOP_PROOF_OUTCOME, confirmed_stop_state, ensure_running_result,
    environment_descriptor, lifecycle_state_is_valid,
};

/// Typed refusal for a lifecycle operation. The handler maps each variant to a
/// status code; no variant mutates persisted state.
#[derive(Debug)]
pub enum LifecycleError {
    /// No owner-scoped environment row exists.
    NotFound,
    /// The caller's expected generation does not match the stored value.
    StaleGeneration { current: i64 },
    /// A replace fence (`expected_container_generation`) does not match.
    StaleCandidate { current: Option<i64> },
    /// A stop proof fence or the proof row itself is not valid.
    ProofMismatch,
    /// The operation token already exists with a different intent.
    TokenConflict,
    /// The operation token is blank or over the bounded length.
    InvalidToken,
    /// A stop was requested for an environment with no provision candidate.
    NotProvisioned,
    /// A provision was requested while a candidate already exists.
    AlreadyProvisioned,
    /// A stop is already pending or claimed.
    StopInFlight,
    /// Unresolved ensure/stop blockers prevent the mutation.
    UnresolvedBlockers,
    /// No manager is enabled, online and recently seen for a first provision.
    ManagerUnavailable,
    /// The transaction itself failed; no lifecycle state mutated, but the
    /// caller must report an internal error.
    Internal(anyhow::Error),
}

impl From<sqlx::Error> for LifecycleError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<anyhow::Error> for LifecycleError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}
