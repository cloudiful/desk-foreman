//! Retained verified repository ownership.
//!
//! The artifact owns the private operation directory and the verified bare
//! repository after a successful verification sequence. It intentionally has
//! no public path accessor, no `Clone`, and no serde authority.

use std::fmt;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

use runner_protocol::{GitVerifyRequest, GitVerifyResult, RunnerOwner};
use uuid::Uuid;

use super::helper::HelperPaths;
use super::verify::{GitVerifyError, Quiescence, remove_tree};

pub(crate) struct VerifiedGitIdentity {
    application_id: i64,
    workspace_binding_id: i64,
    principal_digest: String,
    code_project_id: Uuid,
    authorization_generation: Uuid,
}

impl VerifiedGitIdentity {
    pub(super) fn from_validated_request(
        request: &GitVerifyRequest,
    ) -> Result<Self, GitVerifyError> {
        let RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } = &request.owner
        else {
            return Err(GitVerifyError::InvalidRequest);
        };
        if *workspace_binding_id != request.workspace_binding_id {
            return Err(GitVerifyError::InvalidRequest);
        }
        Ok(Self {
            application_id: *application_id,
            workspace_binding_id: request.workspace_binding_id,
            principal_digest: principal_digest.clone(),
            code_project_id: request.code_project_id,
            authorization_generation: request.authorization_generation,
        })
    }

    pub(crate) fn matches(&self, request: &GitVerifyRequest) -> bool {
        let RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } = &request.owner
        else {
            return false;
        };
        self.application_id == *application_id
            && self.workspace_binding_id == request.workspace_binding_id
            && self.workspace_binding_id == *workspace_binding_id
            && self.principal_digest == *principal_digest
            && self.code_project_id == request.code_project_id
            && self.authorization_generation == request.authorization_generation
    }
}

impl fmt::Debug for VerifiedGitIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGitIdentity")
            .field("application_id", &self.application_id)
            .field("workspace_binding_id", &self.workspace_binding_id)
            .field("code_project_id", &self.code_project_id)
            .field("authorization_generation", &self.authorization_generation)
            .finish_non_exhaustive()
    }
}

pub(crate) struct VerifiedGitRepository {
    parent: OwnedFd,
    name: String,
    repo: OwnedFd,
    summary: GitVerifyResult,
    identity: VerifiedGitIdentity,
    paths: HelperPaths,
    state: Quiescence,
}

impl VerifiedGitRepository {
    pub(super) fn new(
        parent: OwnedFd,
        name: String,
        repo: OwnedFd,
        summary: GitVerifyResult,
        identity: VerifiedGitIdentity,
        paths: HelperPaths,
    ) -> Self {
        Self {
            parent,
            name,
            repo,
            summary,
            identity,
            paths,
            state: Quiescence::Quiescent,
        }
    }

    pub(crate) fn summary(&self) -> &GitVerifyResult {
        &self.summary
    }

    pub(crate) fn identity(&self) -> &VerifiedGitIdentity {
        &self.identity
    }

    /// Internal access for trusted Git authority code only.
    pub(super) fn repo_fd(&self) -> BorrowedFd<'_> {
        self.repo.as_fd()
    }

    /// Internal cross-process helper paths for trusted Git authority code only.
    pub(super) fn helper_paths(&self) -> &HelperPaths {
        &self.paths
    }

    /// Internal mount-root access for trusted Git authority code only.
    pub(super) fn operation_root(&self) -> &Path {
        &self.paths.operation_root
    }

    pub(super) fn begin_operation(&mut self) {
        self.state = Quiescence::NonQuiescent;
    }

    pub(super) fn confirm_quiescent(&mut self) {
        if self.state == Quiescence::NonQuiescent {
            self.state = Quiescence::Quiescent;
        }
    }

    pub(super) fn is_quiescent(&self) -> bool {
        self.state == Quiescence::Quiescent
    }
}

impl Drop for VerifiedGitRepository {
    fn drop(&mut self) {
        if self.is_quiescent() {
            remove_tree(&self.parent, &self.name);
        }
    }
}

impl fmt::Debug for VerifiedGitRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedGitRepository")
            .field("summary", &self.summary)
            .field("identity", &self.identity)
            .field("quiescent", &self.is_quiescent())
            .finish_non_exhaustive()
    }
}
