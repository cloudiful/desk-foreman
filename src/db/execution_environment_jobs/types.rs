//! Focused types for durable environment-operation records (issue 741 P1).
//!
//! A row records one operation intent (ensure or stop) for one exact
//! generation of one execution environment. `token` is an opaque
//! caller-chosen replay key; `result` is a typed, bounded payload; the error
//! summary is a redacted diagnostic string. No dispatch layer consumes rows
//! in this phase, and no HTTP response is added.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Job kinds accepted by the store.
pub const EXECUTION_ENVIRONMENT_JOB_KINDS: [&str; 3] = ["ensure", "stop", "prepare_native_task"];

/// Checked job states.
pub const EXECUTION_ENVIRONMENT_JOB_STATES: [&str; 6] = [
    "pending",
    "claimed",
    "succeeded",
    "failed",
    "uncertain",
    "superseded",
];

/// States that keep an operation active: they are the environment's blocker
/// and forbid a new ensure of any generation. `superseded` is terminal and
/// never a blocker.
pub const EXECUTION_ENVIRONMENT_JOB_ACTIVE_STATES: [&str; 3] = ["pending", "claimed", "uncertain"];

/// Terminal states an operation may record. `superseded` is a stop-proof
/// cancellation of an obsolete ensure record, never an ensure success.
pub const EXECUTION_ENVIRONMENT_JOB_TERMINAL_STATES: [&str; 4] =
    ["succeeded", "failed", "uncertain", "superseded"];

/// Maximum accepted opaque token length.
pub const EXECUTION_ENVIRONMENT_JOB_TOKEN_MAX_LEN: usize = 128;

/// Maximum persisted redacted error-summary length.
pub const EXECUTION_ENVIRONMENT_JOB_ERROR_SUMMARY_MAX_LEN: usize = 512;

/// True when `job_kind` is a checked value.
pub fn job_kind_is_valid(job_kind: &str) -> bool {
    EXECUTION_ENVIRONMENT_JOB_KINDS.contains(&job_kind)
}

/// True when `state` is a checked value.
pub fn job_state_is_valid(state: &str) -> bool {
    EXECUTION_ENVIRONMENT_JOB_STATES.contains(&state)
}

/// True when `state` keeps an operation active.
pub fn job_state_is_active(state: &str) -> bool {
    EXECUTION_ENVIRONMENT_JOB_ACTIVE_STATES.contains(&state)
}

/// True when `state` is a terminal state.
pub fn job_state_is_terminal(state: &str) -> bool {
    EXECUTION_ENVIRONMENT_JOB_TERMINAL_STATES.contains(&state)
}

/// True when `token` is a usable opaque replay key: non-blank after trim and
/// bounded length.
pub fn job_token_is_valid(token: &str) -> bool {
    let trimmed = token.trim();
    !trimmed.is_empty() && trimmed.len() <= EXECUTION_ENVIRONMENT_JOB_TOKEN_MAX_LEN
}

/// Bounded redaction for persisted error summaries. Longer input is
/// truncated on a character boundary with an explicit marker; no structural
/// parsing happens here.
pub fn redact_error_summary(error: &str) -> String {
    const MARKER: &str = "…[truncated]";
    if error.len() <= EXECUTION_ENVIRONMENT_JOB_ERROR_SUMMARY_MAX_LEN {
        return error.to_string();
    }
    let mut end = EXECUTION_ENVIRONMENT_JOB_ERROR_SUMMARY_MAX_LEN;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &error[..end], MARKER)
}

/// One durable environment-operation record.
///
/// `assigned_manager_id` is fixed at enqueue and never changed; it is also the
/// only manager allowed to claim or complete the row.
#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
pub struct ExecutionEnvironmentJob {
    pub job_id: Uuid,
    pub token: String,
    pub environment_id: Uuid,
    pub application_id: i64,
    pub assigned_manager_id: i64,
    pub job_kind: String,
    pub target_generation: i64,
    /// The native task this job serves, when it serves one.
    ///
    /// `None` for every legacy ensure/stop record; set (and required by a check
    /// constraint) for a `prepare_native_task` record.
    pub native_task_id: Option<Uuid>,
    pub state: String,
    pub claimed_at: Option<DateTime<Utc>>,
    pub deadline_at: DateTime<Utc>,
    pub terminal_at: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
    pub error_summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ExecutionEnvironmentJob {
    /// True when this record still blocks a new operation.
    pub fn is_active(&self) -> bool {
        job_state_is_active(&self.state)
    }
}

/// Typed persisted operation result. Only the fields the store needs to
/// record survive; this is not a universal job framework. No host path,
/// volume name, or secret is ever part of it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ExecutionEnvironmentJobResult {
    /// Outcome kind actually persisted (for example `stop_confirmed`).
    pub outcome: String,
    /// Observed container state when the operation recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_state: Option<String>,
    /// Positive stop confirmation; absent for outcomes that have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<bool>,
}

impl ExecutionEnvironmentJobResult {
    /// Result recorded by a successful stop acknowledgment.
    pub fn stop_confirmed(observed_state: &str) -> Self {
        Self {
            outcome: "stop_confirmed".to_string(),
            observed_state: Some(observed_state.to_string()),
            confirmed: Some(true),
        }
    }

    /// Result recorded when a stop could not be positively confirmed.
    pub fn stop_unconfirmed(observed_state: &str) -> Self {
        Self {
            outcome: "stop_unconfirmed".to_string(),
            observed_state: Some(observed_state.to_string()),
            confirmed: Some(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_states_and_terminality_are_checked_values() {
        for kind in EXECUTION_ENVIRONMENT_JOB_KINDS {
            assert!(job_kind_is_valid(kind));
        }
        assert!(!job_kind_is_valid("restart"));
        for state in EXECUTION_ENVIRONMENT_JOB_STATES {
            assert!(job_state_is_valid(state));
        }
        assert!(!job_state_is_valid("running"));

        for state in EXECUTION_ENVIRONMENT_JOB_ACTIVE_STATES {
            assert!(job_state_is_active(state));
        }
        for state in ["pending", "claimed"] {
            assert!(!job_state_is_terminal(state));
        }
        for state in EXECUTION_ENVIRONMENT_JOB_TERMINAL_STATES {
            assert!(job_state_is_terminal(state));
        }
        for state in ["succeeded", "failed"] {
            assert!(!job_state_is_active(state));
        }
        // `uncertain` is both a terminal record and an active blocker.
        assert!(job_state_is_terminal("uncertain"));
        assert!(job_state_is_active("uncertain"));
        // `superseded` is terminal and never an active blocker.
        assert!(job_state_is_valid("superseded"));
        assert!(job_state_is_terminal("superseded"));
        assert!(!job_state_is_active("superseded"));
    }

    #[test]
    fn tokens_are_non_blank_and_bounded() {
        assert!(job_token_is_valid("op-token-1"));
        assert!(!job_token_is_valid("   "));
        assert!(job_token_is_valid(&"x".repeat(128)));
        assert!(!job_token_is_valid(&"x".repeat(129)));
    }

    #[test]
    fn error_summaries_are_redacted_on_char_boundaries() {
        assert_eq!(redact_error_summary("boom"), "boom");
        let long = "e".repeat(600);
        let redacted = redact_error_summary(&long);
        assert!(redacted.len() < long.len());
        assert!(redacted.ends_with("…[truncated]"));

        let multibyte = "é".repeat(600);
        let redacted = redact_error_summary(&multibyte);
        assert!(redacted.ends_with("…[truncated]"));
        // Must never panic on a multi-byte boundary.
        assert!(redacted.starts_with('é'));
    }

    #[test]
    fn result_shapes_round_trip_without_leaking_paths() {
        let result = ExecutionEnvironmentJobResult::stop_confirmed("exited");
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["outcome"], "stop_confirmed");
        assert_eq!(value["confirmed"], true);
        assert!(value.get("host_path").is_none());
        let parsed: ExecutionEnvironmentJobResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, result);

        let unconfirmed = ExecutionEnvironmentJobResult::stop_unconfirmed("running");
        assert_eq!(unconfirmed.confirmed, Some(false));
    }
}
