//! Internal application schemas for native Git gateway discovery and upstream
//! management (issue 1180 P28/P29A).

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError, ValidationErrors};

use crate::api::admin::workspace_git_upstreams::types::{
    CreateWorkspaceGitUpstreamRequest, UpdateWorkspaceGitUpstreamRequest,
};

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkspaceGitUpstreamView {
    pub remote_alias: String,
    pub enabled: bool,
    pub capability_generation: i64,
    pub remote_url: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkspaceGitGatewayResponse {
    pub gateway_base_url: String,
    pub upstreams: Vec<WorkspaceGitUpstreamView>,
}

#[derive(Clone, Deserialize, ToSchema)]
pub struct WorkspaceGitCapabilityRequest {
    pub external_user_id: String,
    #[serde(default)]
    pub lease_owner: Option<String>,
    #[serde(default)]
    pub principal_id: Option<String>,
}

impl validator::Validate for WorkspaceGitCapabilityRequest {
    fn validate(&self) -> Result<(), validator::ValidationErrors> {
        if self.is_valid() {
            return Ok(());
        }
        let mut errors = validator::ValidationErrors::new();
        let mut error = validator::ValidationError::new("validation");
        error.message = Some(std::borrow::Cow::Borrowed("invalid Git capability request"));
        errors.add("request", error);
        Err(errors)
    }
}

impl WorkspaceGitCapabilityRequest {
    pub fn is_valid(&self) -> bool {
        bounded(&self.external_user_id, 256)
            && self
                .lease_owner
                .as_deref()
                .is_none_or(|value| bounded(value, 256))
            && self
                .principal_id
                .as_deref()
                .is_none_or(|value| bounded(value, 512))
    }
}

#[derive(Serialize, ToSchema)]
pub struct WorkspaceGitCapabilityResponse {
    pub capability: String,
    pub expires_at: DateTime<Utc>,
    pub gateway_base_url: String,
    pub upstreams: Vec<WorkspaceGitUpstreamView>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateInternalWorkspaceGitUpstreamRequest {
    pub lease_owner: String,
    #[serde(flatten)]
    pub upstream: CreateWorkspaceGitUpstreamRequest,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateInternalWorkspaceGitUpstreamRequest {
    pub lease_owner: String,
    #[serde(flatten)]
    pub upstream: UpdateWorkspaceGitUpstreamRequest,
}

#[derive(Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeleteInternalWorkspaceGitUpstreamQuery {
    pub lease_owner: String,
}

impl Validate for CreateInternalWorkspaceGitUpstreamRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        check_lease_owner(&self.lease_owner, &mut errors);
        errors.merge_self("upstream", self.upstream.validate());
        finish(errors)
    }
}

impl Validate for UpdateInternalWorkspaceGitUpstreamRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        check_lease_owner(&self.lease_owner, &mut errors);
        errors.merge_self("upstream", self.upstream.validate());
        finish(errors)
    }
}

impl Validate for DeleteInternalWorkspaceGitUpstreamQuery {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        check_lease_owner(&self.lease_owner, &mut errors);
        finish(errors)
    }
}

fn check_lease_owner(value: &str, errors: &mut ValidationErrors) {
    if !bounded(value, 256) {
        errors.add(
            "lease_owner",
            invalid("must be non-empty, at most 256 bytes and free of NUL, CR or LF"),
        );
    }
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
}

fn invalid(message: &'static str) -> ValidationError {
    let mut error = ValidationError::new("validation");
    error.message = Some(Cow::Borrowed(message));
    error
}

fn finish(errors: ValidationErrors) -> Result<(), ValidationErrors> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_requests_are_bounded_and_line_break_free() {
        let valid = WorkspaceGitCapabilityRequest {
            external_user_id: "user-1".into(),
            lease_owner: Some("session-1".into()),
            principal_id: None,
        };
        assert!(valid.is_valid());

        let blank = WorkspaceGitCapabilityRequest {
            external_user_id: String::new(),
            lease_owner: None,
            principal_id: None,
        };
        assert!(!blank.is_valid());

        let injected = WorkspaceGitCapabilityRequest {
            external_user_id: "user\n1".into(),
            lease_owner: None,
            principal_id: None,
        };
        assert!(!injected.is_valid());
    }
}
