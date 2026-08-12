use std::{future::Future, pin::Pin, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub(crate) const MAX_RATIONALE_CHARS: usize = 1_024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Shell,
    Stdin,
    Patch,
    Edit,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    #[default]
    Inherit,
    Disabled,
    Enabled,
}

impl ApprovalMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inherit" => Some(Self::Inherit),
            "disabled" => Some(Self::Disabled),
            "enabled" => Some(Self::Enabled),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewRequest {
    pub action: ReviewAction,
    pub input: String,
    pub workdir: Option<String>,
    pub context: Value,
}

impl ReviewRequest {
    pub fn shell(command: impl Into<String>, workdir: Option<String>, context: Value) -> Self {
        Self {
            action: ReviewAction::Shell,
            input: command.into(),
            workdir,
            context,
        }
    }

    pub fn stdin(chars: impl Into<String>, context: Value) -> Self {
        Self {
            action: ReviewAction::Stdin,
            input: chars.into(),
            workdir: None,
            context,
        }
    }

    pub fn patch(patch: impl Into<String>, context: Value) -> Self {
        Self {
            action: ReviewAction::Patch,
            input: patch.into(),
            workdir: None,
            context,
        }
    }

    pub fn edit(path: impl Into<String>, context: Value) -> Self {
        Self {
            action: ReviewAction::Edit,
            input: path.into(),
            workdir: None,
            context,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecisionKind {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRisk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReviewDecision {
    pub decision: ReviewDecisionKind,
    pub risk: ReviewRisk,
    pub reason_code: String,
    pub rationale: String,
    pub safer_alternative: Option<String>,
}

impl ReviewDecision {
    pub fn permits_execution(&self) -> bool {
        matches!(self.decision, ReviewDecisionKind::Allow)
            && matches!(self.risk, ReviewRisk::Low | ReviewRisk::Medium)
    }

    pub(crate) fn validate(&self) -> Result<(), ApprovalError> {
        if self.reason_code.is_empty() || self.reason_code.len() > 64 {
            return Err(ApprovalError::InvalidResponse);
        }
        if self.rationale.is_empty() || self.rationale.chars().count() > MAX_RATIONALE_CHARS {
            return Err(ApprovalError::InvalidResponse);
        }
        if self
            .safer_alternative
            .as_ref()
            .is_some_and(|value| value.chars().count() > MAX_RATIONALE_CHARS)
        {
            return Err(ApprovalError::InvalidResponse);
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("approval input exceeds the reviewer limit")]
    InputTooLarge,
    #[error("approval reviewer request timed out")]
    TimedOut,
    #[error("approval reviewer is unavailable")]
    Unavailable,
    #[error("approval reviewer returned an invalid response")]
    InvalidResponse,
}

pub type ApprovalFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ReviewDecision, ApprovalError>> + Send + 'a>>;

pub trait ApprovalReviewer: Send + Sync {
    fn provider_name(&self) -> &'static str;
    fn model_identifier(&self) -> Option<&str> {
        None
    }
    fn endpoint_identifier(&self) -> Option<&str> {
        None
    }
    fn review<'a>(&'a self, request: &'a ReviewRequest) -> ApprovalFuture<'a>;
}

impl<T> ApprovalReviewer for Arc<T>
where
    T: ApprovalReviewer + ?Sized,
{
    fn provider_name(&self) -> &'static str {
        (**self).provider_name()
    }

    fn model_identifier(&self) -> Option<&str> {
        (**self).model_identifier()
    }

    fn endpoint_identifier(&self) -> Option<&str> {
        (**self).endpoint_identifier()
    }

    fn review<'a>(&'a self, request: &'a ReviewRequest) -> ApprovalFuture<'a> {
        (**self).review(request)
    }
}
