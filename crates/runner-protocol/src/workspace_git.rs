//! Automatic native Git preparation for ordinary workspace shells (issue 1180 P28).
//!
//! The capability is a short-lived workspace-scoped bearer, not an upstream or
//! admin credential. It travels only in this manager job payload, gets written
//! to a tmpfs file by the manager, and is never placed in argv, `.git/config`,
//! workspace files or logs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::RunnerOwner;

const MAX_CAPABILITY_BYTES: usize = 16 * 1024;
const MAX_ALIASES: usize = 200;

#[derive(Clone, Deserialize, Serialize)]
pub struct GitCapabilitySecret(String);

impl GitCapabilitySecret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_valid(&self) -> bool {
        !self.0.is_empty() && self.0.len() <= MAX_CAPABILITY_BYTES
    }
}

impl std::fmt::Debug for GitCapabilitySecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGitRemote {
    pub remote_alias: String,
    pub url: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareWorkspaceGitRequest {
    pub owner: RunnerOwner,
    pub workspace_root: PathBuf,
    pub network_enabled: bool,
    pub binding_id: i64,
    pub capability: Option<GitCapabilitySecret>,
    pub expires_at: i64,
    pub upstreams: Vec<WorkspaceGitRemote>,
}

impl std::fmt::Debug for PrepareWorkspaceGitRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrepareWorkspaceGitRequest")
            .field("owner", &self.owner)
            .field("workspace_root", &self.workspace_root)
            .field("network_enabled", &self.network_enabled)
            .field("binding_id", &self.binding_id)
            .field("capability", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("upstreams", &self.upstreams)
            .finish()
    }
}

impl PrepareWorkspaceGitRequest {
    pub fn is_valid(&self) -> bool {
        self.binding_id > 0
            && self.workspace_root.is_absolute()
            && (self.capability.is_none() || self.expires_at > 0)
            && (self.capability.is_some() || self.upstreams.is_empty())
            && self.upstreams.len() <= MAX_ALIASES
            && self
                .capability
                .as_ref()
                .is_none_or(GitCapabilitySecret::is_valid)
            && self.upstreams.iter().all(|upstream| {
                valid_alias(&upstream.remote_alias) && valid_remote_url(&upstream.url)
            })
    }
}

fn valid_alias(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_remote_url(value: &str) -> bool {
    value.len() <= 2048
        && !value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        && value.chars().all(|character| !character.is_control())
        && (value.starts_with("http://") || value.starts_with("https://"))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareWorkspaceGitResult {
    pub prepared: bool,
    pub remotes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PrepareWorkspaceGitRequest {
        PrepareWorkspaceGitRequest {
            owner: RunnerOwner::WorkspaceBinding {
                workspace_binding_id: 7,
            },
            workspace_root: PathBuf::from("/srv/workspaces/7"),
            network_enabled: true,
            binding_id: 7,
            capability: Some(GitCapabilitySecret::new("dfgit1.payload.tag".into())),
            expires_at: 1_700_000_000,
            upstreams: vec![WorkspaceGitRemote {
                remote_alias: "origin".into(),
                url: "http://127.0.0.1:3000/git/7/origin".into(),
            }],
        }
    }

    #[test]
    fn capability_debug_is_redacted() {
        let rendered = format!("{:?}", request());
        assert!(!rendered.contains("dfgit1."));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn rejects_escaping_or_unsafe_requests() {
        assert!(request().is_valid());

        let mut bad_alias = request();
        bad_alias.upstreams[0].remote_alias = "../origin".into();
        assert!(!bad_alias.is_valid());

        let mut bad_url = request();
        bad_url.upstreams[0].url = "file:///tmp/repo".into();
        assert!(!bad_url.is_valid());

        let mut relative = request();
        relative.workspace_root = PathBuf::from("relative/workspace");
        assert!(!relative.is_valid());

        let mut oversized = request();
        oversized.capability = Some(GitCapabilitySecret::new(
            "x".repeat(MAX_CAPABILITY_BYTES + 1),
        ));
        assert!(!oversized.is_valid());
    }
}
