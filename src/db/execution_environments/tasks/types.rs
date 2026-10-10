//! Durable native task intent types (issue 753 P1).
//!
//! Split from `tasks.rs` so the persistence entry stays wiring-only. One
//! task is one row: an immutable task UUID owner, the authorized workspace
//! binding it will eventually mount, the subject it was admitted for, the
//! authorization generation it targets, and the immutable manager snapshot it
//! was bound to.
//!
//! The task UUID — not an environment generation — is the writer identity. A
//! generation is a fence, not an owner: it may advance while a task stays
//! active, and an `uncertain` task therefore keeps blocking new intents across
//! every generation until an operator resolves it.

use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

/// Task states that occupy an environment or binding exclusively.
///
/// Exactly the set covered by the partial unique indexes, so the Rust guard
/// and the database can never disagree about what "active" means.
pub const NATIVE_TASK_ACTIVE_STATES: [&str; 6] = [
    "admitted",
    "preparing",
    "prepared",
    "running",
    "stopping",
    "uncertain",
];

/// Every state a task row may hold.
pub const NATIVE_TASK_STATES: [&str; 7] = [
    "admitted",
    "preparing",
    "prepared",
    "running",
    "stopping",
    "uncertain",
    "released",
];

/// Longest accepted operation token, matching the database check.
pub const NATIVE_TASK_TOKEN_MAX_BYTES: usize = 128;
/// Longest accepted subject string, matching the database check.
pub const NATIVE_TASK_SUBJECT_MAX_BYTES: usize = 128;

/// One durable native task intent row.
#[derive(Clone, Debug, FromRow)]
pub struct NativeTask {
    pub task_id: Uuid,
    pub operation_token: String,
    pub environment_id: Uuid,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub subject_external_user_id: String,
    pub target_generation: i64,
    pub assigned_manager_id: i64,
    pub state: String,
    pub container_generation: Option<i64>,
    /// The durable preparation job that materializes this task's checkout,
    /// once one is enqueued. `None` until preparation is scheduled.
    pub prepare_job_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub terminal_at: Option<DateTime<Utc>>,
}

impl NativeTask {
    /// True while this task still occupies its environment and binding.
    pub fn is_active(&self) -> bool {
        NATIVE_TASK_ACTIVE_STATES.contains(&self.state.as_str())
    }

    /// True once this task is terminal and can no longer block anything.
    pub fn is_released(&self) -> bool {
        self.state == "released"
    }
}

/// The owning environment as a task admission sees it.
///
/// Only the columns task admission decides on; existing lifecycle queries keep
/// selecting their own explicit column sets, so adding the active-task anchor
/// changes no other route's read.
#[derive(Clone, Debug, FromRow)]
pub struct TaskEnvironment {
    pub environment_id: Uuid,
    pub application_id: i64,
    pub external_user_id: String,
    pub generation: i64,
    pub state: String,
    pub assigned_manager_id: Option<i64>,
    pub container_generation: Option<i64>,
    pub stop_proof_job_id: Option<Uuid>,
    pub stop_proof_generation: Option<i64>,
    pub active_native_task_id: Option<Uuid>,
}

/// The authorized workspace binding as task admission sees it.
#[derive(Clone, Debug, FromRow)]
pub struct TaskBinding {
    pub workspace_binding_id: i64,
    pub application_id: i64,
    pub external_user_id: String,
    pub workspace_key: String,
    pub is_active: bool,
    pub lifecycle_state: String,
    pub resource_kind: Option<String>,
    pub write_lease_owner: Option<String>,
}

impl TaskBinding {
    /// True when this binding is a live code project resource workspace.
    ///
    /// The fixed external-user marker is deliberately NOT compared against the
    /// subject: authorization is the authenticated application's delegation,
    /// and the marker only exists to keep the shared row unique.
    pub fn is_active_code_project(&self) -> bool {
        self.resource_kind.as_deref() == Some("code_project") && self.is_active
    }

    /// True while any legacy writer holds this binding, expired or not.
    ///
    /// Expiry is never used to infer that no process exists: a stale owner name
    /// still blocks a native intent until an operator clears it.
    pub fn has_legacy_write_owner(&self) -> bool {
        self.write_lease_owner.is_some()
    }

    /// True when the binding lifecycle blocks admission.
    ///
    /// Only the live `active` state admits. `archived` and `resetting` block,
    /// and so does any state this build does not recognise: an unknown state
    /// must fail closed rather than be treated as usable.
    pub fn lifecycle_blocks_admission(&self) -> bool {
        self.lifecycle_state != "active"
    }
}

/// Why a task mutation was refused.
///
/// Closed and prose-free: each variant maps to one documented status, so a
/// refusal can never leak whether a foreign task, binding or proof exists.
#[derive(Debug)]
pub enum NativeTaskError {
    /// The environment, task or binding is absent for this application.
    NotFound,
    /// The caller's generation fence does not match the stored generation.
    StaleGeneration,
    /// The subject does not match the environment's own subject.
    SubjectMismatch,
    /// The binding is not a live code project resource workspace.
    BindingIneligible,
    /// The binding's lifecycle blocks admission (archived, resetting, ...).
    BindingLifecycle,
    /// A legacy write lease owner is present, expired or not.
    LegacyOwnerPresent,
    /// Another task already holds this environment or binding.
    TaskAlreadyActive,
    /// The same operation token was used with a different intent.
    TokenConflict,
    /// A lifecycle operation is unresolved for this environment.
    UnresolvedBlockers,
    /// The environment has no bound manager, or none is usable.
    ManagerUnavailable,
    /// The environment is not in a state that admits a task intent.
    EnvironmentNotIdle,
    /// No enabled+online+recently-seen manager exists to bind.
    NoManagerAvailable,
    /// The task has already been released.
    AlreadyReleased,
    /// The stop proof does not match this task's durable evidence.
    ProofMismatch,
    /// A bounded input was missing or malformed.
    InvalidToken,
    /// A query failed. The text is never surfaced to a caller; it exists only
    /// for the server log.
    Internal,
}

impl NativeTaskError {
    /// Stable, prose-free refusal code for logs and assertions.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::StaleGeneration => "stale_generation",
            Self::SubjectMismatch => "subject_mismatch",
            Self::BindingIneligible => "binding_ineligible",
            Self::BindingLifecycle => "binding_lifecycle",
            Self::LegacyOwnerPresent => "legacy_owner_present",
            Self::TaskAlreadyActive => "task_already_active",
            Self::TokenConflict => "token_conflict",
            Self::UnresolvedBlockers => "unresolved_blockers",
            Self::ManagerUnavailable => "manager_unavailable",
            Self::EnvironmentNotIdle => "environment_not_idle",
            Self::NoManagerAvailable => "no_manager_available",
            Self::AlreadyReleased => "already_released",
            Self::ProofMismatch => "proof_mismatch",
            Self::InvalidToken => "invalid_token",
            Self::Internal => "internal",
        }
    }
}

/// What one caller wants to admit.
#[derive(Clone, Debug)]
pub struct NativeTaskRequest {
    pub task_id: Uuid,
    pub workspace_binding_id: i64,
    pub subject_external_user_id: String,
    pub expected_generation: i64,
    pub operation_token: String,
}

/// The stop proof a release requires, reduced to exactly what is checked.
///
/// A local typed row rather than the shared job record: this phase reads the
/// `native_task_id` link that the new migration adds, and the shared job type
/// is outside this phase's write allowlist.
#[derive(Clone, Debug, FromRow)]
pub struct TaskStopProof {
    pub job_id: Uuid,
    pub environment_id: Uuid,
    pub application_id: i64,
    pub assigned_manager_id: i64,
    pub job_kind: String,
    pub target_generation: i64,
    pub state: String,
    pub native_task_id: Option<Uuid>,
    pub result: Option<serde_json::Value>,
}

/// True when `token` is a usable bounded operation token.
pub fn task_token_is_valid(token: &str) -> bool {
    !token.trim().is_empty() && token.len() <= NATIVE_TASK_TOKEN_MAX_BYTES
}

/// True when `subject` is a usable bounded subject string.
pub fn task_subject_is_valid(subject: &str) -> bool {
    !subject.trim().is_empty() && subject.len() <= NATIVE_TASK_SUBJECT_MAX_BYTES
}

#[cfg(test)]
mod tests {
    use super::{
        NATIVE_TASK_ACTIVE_STATES, NATIVE_TASK_TOKEN_MAX_BYTES, task_subject_is_valid,
        task_token_is_valid,
    };

    #[test]
    fn the_active_state_set_matches_the_partial_unique_indexes() {
        assert_eq!(
            NATIVE_TASK_ACTIVE_STATES,
            [
                "admitted",
                "preparing",
                "prepared",
                "running",
                "stopping",
                "uncertain"
            ],
            "admission guards and the database indexes must agree on \"active\""
        );
        assert!(
            !NATIVE_TASK_ACTIVE_STATES.contains(&"released"),
            "a released task must free its environment and binding"
        );
    }

    #[test]
    fn operation_tokens_are_bounded_and_non_blank() {
        assert!(task_token_is_valid("op-1"));
        assert!(!task_token_is_valid(""));
        assert!(!task_token_is_valid("   "));
        assert!(!task_token_is_valid(
            &"t".repeat(NATIVE_TASK_TOKEN_MAX_BYTES + 1)
        ));
    }

    #[test]
    fn subject_strings_are_bounded_and_non_blank() {
        assert!(task_subject_is_valid("user-1"));
        assert!(!task_subject_is_valid(" "));
        assert!(!task_subject_is_valid(&"u".repeat(129)));
    }
}
