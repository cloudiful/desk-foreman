//! Typed records and admission contracts for durable Git jobs.
//!
//! A row is secret-free and binding-scoped. `token` is an internal operation
//! identifier, never a broker credential. Results are closed typed values, not
//! arbitrary caller JSON.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use thiserror::Error;
use uuid::Uuid;

pub const GIT_JOB_ACTIVE_STATES: [&str; 3] = ["pending", "claimed", "uncertain"];
pub const GIT_JOB_TERMINAL_STATES: [&str; 3] = ["succeeded", "failed", "superseded"];
pub const GIT_JOB_TOKEN_MAX_LEN: usize = 128;
pub const GIT_JOB_BRANCH_MAX_LEN: usize = 255;
pub const GIT_JOB_LEASE_OWNER_MAX_LEN: usize = 256;
pub const GIT_JOB_ERROR_SUMMARY_MAX_LEN: usize = 1024;

pub fn git_job_state_is_active(state: &str) -> bool {
    GIT_JOB_ACTIVE_STATES.contains(&state)
}

pub fn git_job_state_is_terminal(state: &str) -> bool {
    GIT_JOB_TERMINAL_STATES.contains(&state)
}

pub fn is_lower_hex_exact(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn valid_expected_base(value: Option<&str>) -> bool {
    value.is_none_or(|base| is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
}

pub fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= GIT_JOB_TOKEN_MAX_LEN && token.trim() == token
}

pub fn valid_lease_owner(owner: &str) -> bool {
    !owner.is_empty() && owner.len() <= GIT_JOB_LEASE_OWNER_MAX_LEN && owner.trim() == owner
}

#[derive(Clone, Debug, FromRow)]
pub struct GitJobRecord {
    pub job_id: Uuid,
    pub token: String,
    pub operation_id: Uuid,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub assigned_manager_id: i64,
    pub code_project_id: Uuid,
    pub authorization_generation: Uuid,
    pub job_kind: String,
    pub state: String,
    pub canonical_branch: String,
    pub expected_base: Option<String>,
    pub principal_digest: String,
    pub lease_owner: String,
    pub lease_acquired_at: DateTime<Utc>,
    pub reserved_bytes: i64,
    pub cleanup_confirmed: bool,
    pub claimed_at: Option<DateTime<Utc>>,
    pub deadline_at: DateTime<Utc>,
    pub terminal_at: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
    pub error_summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl GitJobRecord {
    pub fn is_active(&self) -> bool {
        git_job_state_is_active(&self.state)
    }
}

#[derive(Clone, Debug)]
pub struct NewGitJob {
    pub job_id: Uuid,
    pub token: String,
    pub operation_id: Uuid,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub assigned_manager_id: i64,
    pub code_project_id: Uuid,
    pub authorization_generation: Uuid,
    pub canonical_branch: String,
    pub expected_base: Option<String>,
    pub lease_owner: String,
    pub lease_acquired_at: DateTime<Utc>,
    pub reserved_bytes: i64,
    pub deadline_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmissionLimits {
    pub max_active: i64,
    pub max_bytes: i64,
    pub max_per_binding: i64,
    pub max_per_manager: i64,
    pub max_bytes_per_manager: i64,
}

impl AdmissionLimits {
    pub fn validate(self) -> Result<Self, AdmissionError> {
        if self.max_active <= 0
            || self.max_bytes <= 0
            || self.max_per_binding <= 0
            || self.max_per_manager <= 0
            || self.max_bytes_per_manager <= 0
        {
            return Err(AdmissionError::InvalidInput);
        }
        Ok(self)
    }
}

#[derive(Debug, Error)]
pub enum AdmissionError {
    #[error("invalid git job admission input")]
    InvalidInput,
    #[error("workspace binding is not available for git admission")]
    BindingUnavailable,
    #[error("workspace binding resource does not match the git request")]
    ResourceMismatch,
    #[error("workspace write lease does not match the git request")]
    LeaseMismatch,
    #[error("workspace write lease has expired")]
    LeaseExpired,
    #[error("git job deadline is invalid")]
    DeadlineInvalid,
    #[error("a native task owns this workspace")]
    NativeTaskHolds,
    #[error("a git job already holds this workspace")]
    BindingBusy,
    #[error("git job capacity is exhausted")]
    CapacityExceeded,
    #[error("git job storage failed")]
    Storage(#[source] anyhow::Error),
}

impl From<sqlx::Error> for AdmissionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.into())
    }
}

impl From<anyhow::Error> for AdmissionError {
    fn from(error: anyhow::Error) -> Self {
        Self::Storage(error)
    }
}

/// Closed typed success proof. No free-form caller JSON is accepted.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitJobResult {
    SyncSucceeded {
        head: String,
        base: Option<String>,
        published: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitJobCompletion {
    Succeeded {
        result: GitJobResult,
    },
    Failed {
        error_summary: String,
        cleanup_confirmed: bool,
    },
}

impl GitJobCompletion {
    pub fn state(&self) -> &'static str {
        match self {
            Self::Succeeded { .. } => "succeeded",
            Self::Failed {
                cleanup_confirmed: true,
                ..
            } => "failed",
            Self::Failed {
                cleanup_confirmed: false,
                ..
            } => "uncertain",
        }
    }

    pub fn cleanup_confirmed(&self) -> bool {
        match self {
            Self::Succeeded { .. } => true,
            Self::Failed {
                cleanup_confirmed, ..
            } => *cleanup_confirmed,
        }
    }

    pub fn result_value(&self) -> anyhow::Result<Option<serde_json::Value>> {
        match self {
            Self::Succeeded { result } => {
                let GitJobResult::SyncSucceeded {
                    head,
                    base,
                    published,
                } = result;
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                    anyhow::bail!("invalid git job result head");
                }
                if let Some(base) = base
                    && !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
                {
                    anyhow::bail!("invalid git job result base");
                }
                if !published {
                    anyhow::bail!("git job result is not published");
                }
                Ok(Some(serde_json::to_value(result)?))
            }
            Self::Failed { .. } => Ok(None),
        }
    }

    pub fn error_summary(&self) -> Option<String> {
        match self {
            Self::Succeeded { .. } => None,
            Self::Failed { error_summary, .. } => Some(redact_error_summary(error_summary)),
        }
    }
}

pub fn redact_error_summary(error: &str) -> String {
    if error.len() <= GIT_JOB_ERROR_SUMMARY_MAX_LEN {
        return error.to_owned();
    }
    let mut end = GIT_JOB_ERROR_SUMMARY_MAX_LEN;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &error[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_bounds_and_expected_base_are_closed() {
        assert!(valid_token("job-token"));
        assert!(!valid_token(""));
        assert!(!valid_token(" job-token"));
        assert!(valid_lease_owner("manager-1"));
        assert!(!valid_lease_owner("manager-1 "));
        assert!(valid_expected_base(None));
        assert!(valid_expected_base(Some(&"a".repeat(40))));
        assert!(valid_expected_base(Some(&"b".repeat(64))));
        assert!(!valid_expected_base(Some(&"A".repeat(40))));
    }

    #[test]
    fn completion_maps_cleanup_uncertainty_to_uncertain() {
        let failed = GitJobCompletion::Failed {
            error_summary: "x".repeat(GIT_JOB_ERROR_SUMMARY_MAX_LEN + 10),
            cleanup_confirmed: true,
        };
        assert_eq!(failed.state(), "failed");
        assert!(failed.error_summary().is_some());
        let uncertain = GitJobCompletion::Failed {
            error_summary: "error".to_owned(),
            cleanup_confirmed: false,
        };
        assert_eq!(uncertain.state(), "uncertain");
    }
}
