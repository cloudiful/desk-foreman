//! Closed Docker plans for one isolated Git clone preparation.
//!
//! Plans are built only from manager configuration, claimed job identity and
//! generated operation names. They contain no credential and no arbitrary
//! caller argument. Execution is an injected typed trait so tests use a fake
//! executor and production uses the narrow Docker CLI runner.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use runner_protocol::{GitObjectFormat, is_canonical_branch};
use tokio::sync::watch;

use crate::config::{RunnerBackendKind, RunnerManagerConfig};

use super::clone_volume::GitVolumePlan;
use super::runtime_config::validate_private_root;

pub(crate) const CONTROL_TARGET: &str = "/git-control";
pub(crate) const CREDENTIAL_TARGET: &str = "/git-cred";
pub(crate) const VOLUME_TARGET: &str = "/git-fetch";
pub(crate) const REPOSITORY_TARGET: &str = "/git-fetch/repo";
pub(crate) const BUNDLE_TARGET: &str = "/git-bundle/bundle.git";
pub(crate) const INSTALL_VOLUME_TARGET: &str = "/git-install";
pub(crate) const INSTALL_REPOSITORY_TARGET: &str = "/git-install/repo";
pub(crate) const WORKSPACE_TARGET: &str = "/git-workspace";
pub(crate) const MAX_BUNDLE_CEILING_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const DIAGNOSTICS_LIMIT_BYTES: usize = 65536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitCloneProfile {
    Proof,
    NetLsRemote,
    FetchInit,
    Fetch,
    Verify,
    InstallClone,
    InstallVerify,
    InstallExport,
    WorkspaceInspect,
    WorkspaceUpdate,
}

impl GitCloneProfile {
    pub(crate) fn mount_slots(self) -> &'static [GitMountSlot] {
        match self {
            Self::Proof => &[GitMountSlot::ControlRo],
            Self::NetLsRemote => &[GitMountSlot::ControlRo, GitMountSlot::CredentialRo],
            Self::FetchInit => &[GitMountSlot::ControlRo, GitMountSlot::VolumeRw],
            Self::Fetch => &[
                GitMountSlot::ControlRo,
                GitMountSlot::CredentialRo,
                GitMountSlot::VolumeRw,
            ],
            Self::Verify => &[GitMountSlot::ControlRo, GitMountSlot::VolumeRo],
            Self::InstallClone => &[GitMountSlot::BundleRo, GitMountSlot::InstallVolumeRw],
            Self::InstallVerify | Self::InstallExport => &[GitMountSlot::InstallVolumeRo],
            Self::WorkspaceInspect => &[GitMountSlot::WorkspaceRo],
            Self::WorkspaceUpdate => &[GitMountSlot::BundleRo, GitMountSlot::WorkspaceRw],
        }
    }

    pub(crate) fn uses_network(self) -> bool {
        matches!(self, Self::NetLsRemote | Self::Fetch)
    }

    pub(crate) fn uses_credential(self) -> bool {
        matches!(self, Self::NetLsRemote | Self::Fetch)
    }

    fn install(self) -> bool {
        matches!(
            self,
            Self::InstallClone | Self::InstallVerify | Self::InstallExport
        )
    }

    fn uses_bundle(self) -> bool {
        matches!(self, Self::InstallClone | Self::WorkspaceUpdate)
    }

    fn workspace(self) -> bool {
        matches!(self, Self::WorkspaceInspect | Self::WorkspaceUpdate)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitMountSlot {
    ControlRo,
    CredentialRo,
    VolumeRw,
    VolumeRo,
    BundleRo,
    InstallVolumeRw,
    InstallVolumeRo,
    WorkspaceRw,
    WorkspaceRo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GitCloneCommand {
    Proof {
        proof_name: String,
    },
    NetLsRemote {
        remote_url: String,
        branch: String,
    },
    FetchInit {
        object_format: GitObjectFormat,
        branch: String,
    },
    Fetch {
        remote_url: String,
        branch: String,
    },
    Verify {
        head: String,
        expected_base: Option<String>,
    },
    ShowObjectFormat,
    Fsck,
    ListTreeSizes {
        head: String,
    },
    ListObjectSizes,
    MergeBaseIsAncestor {
        base: String,
        head: String,
    },
    ReadLocalRef {
        branch: String,
    },
    CatFileCommit {
        head: String,
    },
    BundleCreate {
        branch: String,
    },
    InstallClone {
        branch: String,
    },
    InstallRemoveOrigin,
    InstallCheckout {
        branch: String,
    },
    InstallShowObjectFormat,
    InstallFsck,
    InstallVerifyHead {
        head: String,
    },
    InstallReadBranchRef {
        branch: String,
    },
    InstallStatusClean,
    InstallExport,
    WorkspaceHead,
    WorkspaceBranch,
    WorkspaceStatus,
    WorkspaceImportBundle,
    WorkspaceIsAncestor {
        base: String,
        head: String,
    },
    WorkspaceCommitExists {
        head: String,
    },
    WorkspaceReadTree {
        old: String,
        new: String,
    },
    WorkspaceUpdateRef {
        branch: String,
        old: String,
        new: String,
    },
}

impl GitCloneCommand {
    pub(crate) fn profile(&self) -> GitCloneProfile {
        match self {
            Self::Proof { .. } => GitCloneProfile::Proof,
            Self::NetLsRemote { .. } => GitCloneProfile::NetLsRemote,
            Self::FetchInit { .. } => GitCloneProfile::FetchInit,
            Self::Fetch { .. } => GitCloneProfile::Fetch,
            Self::Verify { .. }
            | Self::ShowObjectFormat
            | Self::Fsck
            | Self::ListTreeSizes { .. }
            | Self::ListObjectSizes
            | Self::MergeBaseIsAncestor { .. }
            | Self::ReadLocalRef { .. }
            | Self::CatFileCommit { .. }
            | Self::BundleCreate { .. } => GitCloneProfile::Verify,
            Self::InstallClone { .. }
            | Self::InstallRemoveOrigin
            | Self::InstallCheckout { .. } => GitCloneProfile::InstallClone,
            Self::InstallShowObjectFormat
            | Self::InstallFsck
            | Self::InstallVerifyHead { .. }
            | Self::InstallReadBranchRef { .. }
            | Self::InstallStatusClean => GitCloneProfile::InstallVerify,
            Self::InstallExport => GitCloneProfile::InstallExport,
            Self::WorkspaceHead | Self::WorkspaceBranch | Self::WorkspaceStatus => {
                GitCloneProfile::WorkspaceInspect
            }
            Self::WorkspaceImportBundle
            | Self::WorkspaceIsAncestor { .. }
            | Self::WorkspaceCommitExists { .. }
            | Self::WorkspaceReadTree { .. }
            | Self::WorkspaceUpdateRef { .. } => GitCloneProfile::WorkspaceUpdate,
        }
    }

    fn argv(&self) -> Vec<String> {
        match self {
            Self::Proof { proof_name } => vec![
                "git".to_owned(),
                "hash-object".to_owned(),
                "--no-filters".to_owned(),
                format!("{CONTROL_TARGET}/{proof_name}"),
            ],
            Self::NetLsRemote { remote_url, branch } => vec![
                "git".to_owned(),
                "-c".to_owned(),
                "http.followRedirects=false".to_owned(),
                "-c".to_owned(),
                "http.proxy=".to_owned(),
                "ls-remote".to_owned(),
                "--heads".to_owned(),
                remote_url.clone(),
                format!("refs/heads/{branch}"),
            ],
            Self::FetchInit {
                object_format,
                branch,
            } => vec![
                "git".to_owned(),
                "init".to_owned(),
                "--bare".to_owned(),
                format!("--object-format={}", object_format.git_name()),
                format!("--initial-branch={branch}"),
                "--ref-format=files".to_owned(),
                REPOSITORY_TARGET.to_owned(),
            ],
            Self::Fetch { remote_url, branch } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "-c".to_owned(),
                "http.followRedirects=false".to_owned(),
                "-c".to_owned(),
                "http.proxy=".to_owned(),
                "-c".to_owned(),
                "protocol.version=2".to_owned(),
                "fetch".to_owned(),
                "--no-tags".to_owned(),
                "--no-recurse-submodules".to_owned(),
                remote_url.clone(),
                format!("refs/heads/{branch}:refs/heads/{branch}"),
            ],
            Self::Verify { head, .. } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                format!("{head}^{{commit}}"),
            ],
            Self::ShowObjectFormat => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--show-object-format".to_owned(),
            ],
            Self::Fsck => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "fsck".to_owned(),
                "--strict".to_owned(),
                "--no-dangling".to_owned(),
            ],
            Self::ListTreeSizes { head } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "ls-tree".to_owned(),
                "-r".to_owned(),
                "-l".to_owned(),
                "-z".to_owned(),
                head.clone(),
            ],
            Self::ListObjectSizes => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "cat-file".to_owned(),
                "--batch-all-objects".to_owned(),
                "--batch-check".to_owned(),
            ],
            Self::MergeBaseIsAncestor { base, head } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "merge-base".to_owned(),
                "--is-ancestor".to_owned(),
                base.clone(),
                head.clone(),
            ],
            Self::ReadLocalRef { branch } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                format!("refs/heads/{branch}"),
            ],
            Self::CatFileCommit { head } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "cat-file".to_owned(),
                "-e".to_owned(),
                format!("{head}^{{commit}}"),
            ],
            Self::BundleCreate { branch } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                REPOSITORY_TARGET.to_owned(),
                "bundle".to_owned(),
                "create".to_owned(),
                "-q".to_owned(),
                "-".to_owned(),
                format!("refs/heads/{branch}"),
            ],
            Self::InstallClone { branch } => vec![
                "git".to_owned(),
                "-c".to_owned(),
                "core.hooksPath=/dev/null".to_owned(),
                "-c".to_owned(),
                "core.fsmonitor=false".to_owned(),
                "clone".to_owned(),
                "--no-hardlinks".to_owned(),
                "--single-branch".to_owned(),
                "--branch".to_owned(),
                branch.clone(),
                BUNDLE_TARGET.to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
            ],
            Self::InstallRemoveOrigin => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "remote".to_owned(),
                "remove".to_owned(),
                "origin".to_owned(),
            ],
            Self::InstallCheckout { branch } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "checkout".to_owned(),
                branch.clone(),
            ],
            Self::InstallShowObjectFormat => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--show-object-format".to_owned(),
            ],
            Self::InstallFsck => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "fsck".to_owned(),
                "--strict".to_owned(),
                "--no-dangling".to_owned(),
            ],
            Self::InstallVerifyHead { head } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                format!("{head}^{{commit}}"),
            ],
            Self::InstallReadBranchRef { branch } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                format!("refs/heads/{branch}"),
            ],
            Self::InstallStatusClean => vec![
                "git".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "status".to_owned(),
                "--porcelain=v1".to_owned(),
                "--untracked-files=all".to_owned(),
            ],
            Self::InstallExport => vec![
                "tar".to_owned(),
                "-C".to_owned(),
                INSTALL_REPOSITORY_TARGET.to_owned(),
                "-cf".to_owned(),
                "-".to_owned(),
                ".".to_owned(),
            ],
            Self::WorkspaceHead => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                "HEAD".to_owned(),
            ],
            Self::WorkspaceBranch => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "symbolic-ref".to_owned(),
                "--quiet".to_owned(),
                "--short".to_owned(),
                "HEAD".to_owned(),
            ],
            Self::WorkspaceStatus => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "status".to_owned(),
                "--porcelain=v1".to_owned(),
                "--untracked-files=all".to_owned(),
            ],
            Self::WorkspaceImportBundle => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "bundle".to_owned(),
                "unbundle".to_owned(),
                BUNDLE_TARGET.to_owned(),
            ],
            Self::WorkspaceIsAncestor { base, head } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "merge-base".to_owned(),
                "--is-ancestor".to_owned(),
                base.clone(),
                head.clone(),
            ],
            Self::WorkspaceCommitExists { head } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "cat-file".to_owned(),
                "-e".to_owned(),
                format!("{head}^{{commit}}"),
            ],
            Self::WorkspaceReadTree { old, new } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "read-tree".to_owned(),
                "-u".to_owned(),
                "-m".to_owned(),
                old.clone(),
                new.clone(),
            ],
            Self::WorkspaceUpdateRef { branch, old, new } => vec![
                "git".to_owned(),
                "-C".to_owned(),
                WORKSPACE_TARGET.to_owned(),
                "update-ref".to_owned(),
                format!("refs/heads/{branch}"),
                new.clone(),
                old.clone(),
            ],
        }
    }

    fn validate_shape(&self) -> Result<(), GitPlanError> {
        match self {
            Self::Proof { proof_name } => {
                if !is_safe_component(proof_name) || !proof_name.starts_with("mount-proof-") {
                    return Err(GitPlanError::Command);
                }
            }
            Self::NetLsRemote { remote_url, branch } | Self::Fetch { remote_url, branch } => {
                if !is_canonical_branch(branch)
                    || remote_url.is_empty()
                    || remote_url.len() > 2048
                    || remote_url.chars().any(|character| {
                        character == '\0' || character == '\n' || character == '\r'
                    })
                {
                    return Err(GitPlanError::Command);
                }
            }
            Self::FetchInit {
                object_format,
                branch,
            } => {
                let _ = object_format;
                if !is_canonical_branch(branch) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::Verify {
                head,
                expected_base,
            } => {
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                    return Err(GitPlanError::Command);
                }
                if let Some(base) = expected_base
                    && !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
                {
                    return Err(GitPlanError::Command);
                }
            }
            Self::WorkspaceHead
            | Self::WorkspaceBranch
            | Self::WorkspaceStatus
            | Self::WorkspaceImportBundle => {}
            Self::WorkspaceIsAncestor { base, head } => {
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64))
                    || !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
                {
                    return Err(GitPlanError::Command);
                }
            }
            Self::WorkspaceCommitExists { head } => {
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::WorkspaceReadTree { old, new } => {
                if !is_lower_hex_exact(old, 40) && !is_lower_hex_exact(old, 64)
                    || !is_lower_hex_exact(new, 40) && !is_lower_hex_exact(new, 64)
                {
                    return Err(GitPlanError::Command);
                }
            }
            Self::WorkspaceUpdateRef { branch, old, new } => {
                if !is_canonical_branch(branch)
                    || !is_lower_hex_exact(old, 40) && !is_lower_hex_exact(old, 64)
                    || !is_lower_hex_exact(new, 40) && !is_lower_hex_exact(new, 64)
                {
                    return Err(GitPlanError::Command);
                }
            }
            Self::ListTreeSizes { head } => {
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::MergeBaseIsAncestor { base, head } => {
                if !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
                    || !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64))
                {
                    return Err(GitPlanError::Command);
                }
            }
            Self::ReadLocalRef { branch } => {
                if !is_canonical_branch(branch) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::CatFileCommit { head } => {
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::BundleCreate { branch } => {
                if !is_canonical_branch(branch) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::InstallClone { branch }
            | Self::InstallCheckout { branch }
            | Self::InstallReadBranchRef { branch } => {
                if !is_canonical_branch(branch) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::InstallVerifyHead { head } => {
                if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                    return Err(GitPlanError::Command);
                }
            }
            Self::ShowObjectFormat
            | Self::Fsck
            | Self::ListObjectSizes
            | Self::InstallRemoveOrigin
            | Self::InstallShowObjectFormat
            | Self::InstallFsck
            | Self::InstallStatusClean
            | Self::InstallExport => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitClonePaths {
    pub(crate) control_dir: PathBuf,
    pub(crate) credential_dir: PathBuf,
    pub(crate) volume_name: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct GitCloneLimits {
    pub(crate) image: String,
    pub(crate) network: String,
    pub(crate) broker_host: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) fetch_volume_bytes: u64,
    pub(crate) bundle_bytes: u64,
    pub(crate) logical_object_bytes: u64,
    pub(crate) checkout_bytes: u64,
    pub(crate) tree_entries: u64,
    pub(crate) deadline: Duration,
    pub(crate) memory_limit: String,
    pub(crate) cpu_limit: String,
    pub(crate) pids_limit: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitCloneUnavailableReason {
    BackendUnsupported,
    ImageMissing,
    ImageNotPinned,
    PrivateRoot,
    HostRootMissing,
    HostRootInvalid,
    LimitsInvalid,
    NetworkInvalid,
    BrokerHostInvalid,
}

impl std::fmt::Display for GitCloneUnavailableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::BackendUnsupported => "git clone backend is unsupported",
            Self::ImageMissing => "git clone image is missing",
            Self::ImageNotPinned => "git clone image is not digest-pinned",
            Self::PrivateRoot => "git clone private root is invalid",
            Self::HostRootMissing => "git clone host private root is missing",
            Self::HostRootInvalid => "git clone host private root is invalid",
            Self::LimitsInvalid => "git clone limits are invalid",
            Self::NetworkInvalid => "git clone network is invalid",
            Self::BrokerHostInvalid => "git clone broker host is invalid",
        })
    }
}

impl std::error::Error for GitCloneUnavailableReason {}

#[derive(Clone, Debug)]
pub(crate) struct GitCloneConfig {
    pub(crate) private_root: PathBuf,
    pub(crate) host_private_root: PathBuf,
    pub(crate) limits: GitCloneLimits,
}

impl GitCloneConfig {
    pub(crate) fn from_runner(
        config: &RunnerManagerConfig,
    ) -> Result<Self, GitCloneUnavailableReason> {
        if config.backend != RunnerBackendKind::Docker {
            return Err(GitCloneUnavailableReason::BackendUnsupported);
        }
        let image = config
            .git_image
            .as_deref()
            .ok_or(GitCloneUnavailableReason::ImageMissing)?;
        if !is_pinned_image(image) {
            return Err(GitCloneUnavailableReason::ImageNotPinned);
        }
        let metadata =
            std::fs::metadata("/proc/self").map_err(|_| GitCloneUnavailableReason::PrivateRoot)?;
        use std::os::unix::fs::MetadataExt;
        let uid = metadata.uid();
        let gid = metadata.gid();
        if uid == 0 {
            return Err(GitCloneUnavailableReason::PrivateRoot);
        }
        let private_root = config
            .git_private_root
            .as_deref()
            .ok_or(GitCloneUnavailableReason::PrivateRoot)?;
        let private_root = validate_private_root(private_root, uid)
            .map_err(|_| GitCloneUnavailableReason::PrivateRoot)?;
        let host_private_root = match config.git_host_private_root.as_deref() {
            Some(root) => {
                if !root.is_absolute()
                    || root
                        .to_string_lossy()
                        .chars()
                        .any(|character| character == '\0' || character == '\n')
                {
                    return Err(GitCloneUnavailableReason::HostRootInvalid);
                }
                root.to_path_buf()
            }
            None if config.docker_host.is_some() => {
                return Err(GitCloneUnavailableReason::HostRootMissing);
            }
            None => private_root.clone(),
        };
        let network = config
            .git_network
            .as_deref()
            .ok_or(GitCloneUnavailableReason::NetworkInvalid)?;
        if !is_valid_network(network) {
            return Err(GitCloneUnavailableReason::NetworkInvalid);
        }
        let broker_host = config
            .git_broker_host
            .as_deref()
            .ok_or(GitCloneUnavailableReason::BrokerHostInvalid)?;
        if !is_valid_broker_host(broker_host) {
            return Err(GitCloneUnavailableReason::BrokerHostInvalid);
        }
        let fetch_volume_bytes = config
            .git_max_fetch_volume_bytes
            .filter(|value| *value > 0)
            .ok_or(GitCloneUnavailableReason::LimitsInvalid)?;
        let bundle_bytes = config
            .git_max_bundle_bytes
            .filter(|value| *value > 0 && *value <= MAX_BUNDLE_CEILING_BYTES)
            .ok_or(GitCloneUnavailableReason::LimitsInvalid)?;
        let logical_object_bytes = config
            .git_max_logical_object_bytes
            .filter(|value| *value > 0)
            .ok_or(GitCloneUnavailableReason::LimitsInvalid)?;
        let checkout_bytes = config
            .git_max_checkout_bytes
            .filter(|value| *value > 0)
            .ok_or(GitCloneUnavailableReason::LimitsInvalid)?;
        let tree_entries = config
            .git_max_tree_entries
            .filter(|value| *value > 0)
            .ok_or(GitCloneUnavailableReason::LimitsInvalid)?;
        if config.max_timeout_ms == 0
            || config.pids_limit == 0
            || !safe_nonempty(&config.memory_limit)
            || !safe_nonempty(&config.cpu_limit)
        {
            return Err(GitCloneUnavailableReason::LimitsInvalid);
        }
        Ok(Self {
            private_root,
            host_private_root,
            limits: GitCloneLimits {
                image: image.to_owned(),
                network: network.to_owned(),
                broker_host: broker_host.to_owned(),
                uid,
                gid,
                fetch_volume_bytes,
                bundle_bytes,
                logical_object_bytes,
                checkout_bytes,
                tree_entries,
                deadline: Duration::from_millis(config.max_timeout_ms),
                memory_limit: config.memory_limit.clone(),
                cpu_limit: config.cpu_limit.clone(),
                pids_limit: config.pids_limit,
            },
        })
    }

    /// Conservative operation reservation: fetch cap + bundle cap + margin.
    pub(crate) fn required_reservation(&self) -> Option<u64> {
        self.limits
            .fetch_volume_bytes
            .checked_add(self.limits.bundle_bytes)?
            .checked_add(65536)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitPlan {
    pub(crate) profile: GitCloneProfile,
    pub(crate) command: GitCloneCommand,
    docker_args: Vec<String>,
    cleanup_args: Vec<String>,
    stdout_limit: usize,
    stderr_limit: usize,
    combined_limit: usize,
    deadline: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitPlanError {
    Command,
    Profile,
    Limits,
    Mounts,
    Network,
    Image,
    Resource,
}

impl std::fmt::Display for GitPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Command => "git clone command is invalid",
            Self::Profile => "git clone profile is invalid",
            Self::Limits => "git clone plan limits are invalid",
            Self::Mounts => "git clone plan mounts are invalid",
            Self::Network => "git clone plan network is invalid",
            Self::Image => "git clone plan image is invalid",
            Self::Resource => "git clone plan resources are invalid",
        })
    }
}

impl std::error::Error for GitPlanError {}

impl GitPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        command: GitCloneCommand,
        paths: &GitClonePaths,
        limits: &GitCloneLimits,
        container_name: String,
        deadline: Duration,
        stdout_limit: usize,
        stderr_limit: usize,
    ) -> Result<Self, GitPlanError> {
        build_plan(
            command,
            MountSource::Clone(paths),
            limits,
            container_name,
            deadline,
            stdout_limit,
            stderr_limit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_install(
        command: GitCloneCommand,
        paths: &GitInstallPaths,
        limits: &GitCloneLimits,
        container_name: String,
        deadline: Duration,
        stdout_limit: usize,
        stderr_limit: usize,
    ) -> Result<Self, GitPlanError> {
        let profile = command.profile();
        if !profile.install() {
            return Err(GitPlanError::Profile);
        }
        build_plan(
            command,
            MountSource::Install(paths),
            limits,
            container_name,
            deadline,
            stdout_limit,
            stderr_limit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_workspace(
        command: GitCloneCommand,
        paths: &GitWorkspacePaths,
        limits: &GitCloneLimits,
        container_name: String,
        deadline: Duration,
        stdout_limit: usize,
        stderr_limit: usize,
    ) -> Result<Self, GitPlanError> {
        let profile = command.profile();
        if !profile.workspace() {
            return Err(GitPlanError::Profile);
        }
        build_plan(
            command,
            MountSource::Workspace(paths),
            limits,
            container_name,
            deadline,
            stdout_limit,
            stderr_limit,
        )
    }

    pub(crate) fn docker_args(&self) -> &[String] {
        &self.docker_args
    }

    /// The argv actually passed to the Docker CLI (the leading `docker`
    /// placeholder is removed because the executor supplies the executable).
    pub(crate) fn execution_args(&self) -> &[String] {
        &self.docker_args[1..]
    }

    pub(crate) fn cleanup_args(&self) -> &[String] {
        &self.cleanup_args
    }

    pub(crate) fn stdout_limit(&self) -> usize {
        self.stdout_limit
    }

    pub(crate) fn stderr_limit(&self) -> usize {
        self.stderr_limit
    }

    pub(crate) fn combined_limit(&self) -> usize {
        self.combined_limit
    }

    pub(crate) fn deadline(&self) -> Duration {
        self.deadline
    }

    pub(crate) fn validate(&self) -> Result<(), GitPlanError> {
        self.command.validate_shape()?;
        if self.command.profile() != self.profile {
            return Err(GitPlanError::Profile);
        }
        let mounts: Vec<&String> = self
            .docker_args
            .iter()
            .skip(1)
            .filter(|argument| *argument == "--mount")
            .collect();
        if mounts.len() != self.profile.mount_slots().len() {
            return Err(GitPlanError::Mounts);
        }
        if self
            .docker_args
            .iter()
            .any(|argument| argument.starts_with("--network="))
            == self.profile.uses_network()
        {
            // Uses-network profiles have a separate --network <name> pair.
            if self.profile.uses_network()
                && !self
                    .docker_args
                    .iter()
                    .any(|argument| argument == "--network")
            {
                return Err(GitPlanError::Network);
            }
        }
        for forbidden in [
            "--privileged",
            "--device",
            "--env-file",
            "--pid",
            "--ipc",
            "--userns",
            "--hostname",
            "--ip",
            "--add-host",
            "--volume",
            "-v",
        ] {
            if self.docker_args.iter().any(|argument| {
                argument == forbidden || argument.starts_with(&format!("{forbidden}="))
            }) {
                return Err(GitPlanError::Mounts);
            }
        }
        if self
            .docker_args
            .iter()
            .filter(|argument| *argument == "--mount")
            .count()
            != self.profile.mount_slots().len()
        {
            return Err(GitPlanError::Mounts);
        }
        let credential_mount = self
            .docker_args
            .iter()
            .any(|argument| argument.contains(&format!("target={CREDENTIAL_TARGET}")));
        if credential_mount != self.profile.uses_credential() {
            return Err(GitPlanError::Mounts);
        }
        let bundle_mount = self
            .docker_args
            .iter()
            .any(|argument| argument.contains(&format!("target={BUNDLE_TARGET}")));
        if bundle_mount != self.profile.uses_bundle() {
            return Err(GitPlanError::Mounts);
        }
        let install_volume_mount = self
            .docker_args
            .iter()
            .any(|argument| argument.contains(&format!("target={INSTALL_VOLUME_TARGET}")));
        if install_volume_mount != self.profile.install() {
            return Err(GitPlanError::Mounts);
        }
        if self.stdout_limit == 0
            || self.stderr_limit == 0
            || self.combined_limit
                != self
                    .stdout_limit
                    .checked_add(self.stderr_limit)
                    .ok_or(GitPlanError::Limits)?
        {
            return Err(GitPlanError::Limits);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitInstallPaths {
    pub(crate) bundle_path: PathBuf,
    pub(crate) volume_name: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct GitWorkspacePaths {
    pub(crate) workspace_path: PathBuf,
    pub(crate) bundle_path: Option<PathBuf>,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

#[derive(Clone, Copy)]
enum MountSource<'a> {
    Clone(&'a GitClonePaths),
    Install(&'a GitInstallPaths),
    Workspace(&'a GitWorkspacePaths),
}

impl MountSource<'_> {
    fn uid(self) -> u32 {
        match self {
            Self::Clone(paths) => paths.uid,
            Self::Install(paths) => paths.uid,
            Self::Workspace(paths) => paths.uid,
        }
    }

    fn gid(self) -> u32 {
        match self {
            Self::Clone(paths) => paths.gid,
            Self::Install(paths) => paths.gid,
            Self::Workspace(paths) => paths.gid,
        }
    }

    fn mount(self, slot: GitMountSlot, profile: GitCloneProfile) -> Result<String, GitPlanError> {
        let mount = match (self, slot) {
            (Self::Clone(paths), GitMountSlot::ControlRo) => format!(
                "type=bind,source={},target={CONTROL_TARGET},ro",
                paths.control_dir.display()
            ),
            (Self::Clone(paths), GitMountSlot::CredentialRo) => {
                if !profile.uses_credential() {
                    return Err(GitPlanError::Mounts);
                }
                format!(
                    "type=bind,source={},target={CREDENTIAL_TARGET},ro",
                    paths.credential_dir.display()
                )
            }
            (Self::Clone(paths), GitMountSlot::VolumeRw | GitMountSlot::VolumeRo) => {
                let mode = if matches!(slot, GitMountSlot::VolumeRo) {
                    ",ro"
                } else {
                    ""
                };
                format!(
                    "type=volume,source={},target={VOLUME_TARGET}{mode}",
                    paths.volume_name
                )
            }
            (Self::Install(paths), GitMountSlot::BundleRo) => {
                if !profile.uses_bundle() {
                    return Err(GitPlanError::Mounts);
                }
                format!(
                    "type=bind,source={},target={BUNDLE_TARGET},ro",
                    paths.bundle_path.display()
                )
            }
            (Self::Workspace(paths), GitMountSlot::WorkspaceRo | GitMountSlot::WorkspaceRw) => {
                let mode = if matches!(slot, GitMountSlot::WorkspaceRo) {
                    ",ro"
                } else {
                    ""
                };
                format!(
                    "type=bind,source={},target={WORKSPACE_TARGET}{mode}",
                    paths.workspace_path.display()
                )
            }
            (Self::Workspace(paths), GitMountSlot::BundleRo) => {
                if !profile.uses_bundle() {
                    return Err(GitPlanError::Mounts);
                }
                let bundle = paths.bundle_path.as_ref().ok_or(GitPlanError::Mounts)?;
                format!(
                    "type=bind,source={},target={BUNDLE_TARGET},ro",
                    bundle.display()
                )
            }
            (
                Self::Install(paths),
                GitMountSlot::InstallVolumeRw | GitMountSlot::InstallVolumeRo,
            ) => {
                let mode = if matches!(slot, GitMountSlot::InstallVolumeRo) {
                    ",ro"
                } else {
                    ""
                };
                format!(
                    "type=volume,source={},target={INSTALL_VOLUME_TARGET}{mode}",
                    paths.volume_name
                )
            }
            _ => return Err(GitPlanError::Mounts),
        };
        Ok(mount)
    }
}

fn build_plan(
    command: GitCloneCommand,
    source: MountSource<'_>,
    limits: &GitCloneLimits,
    container_name: String,
    deadline: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<GitPlan, GitPlanError> {
    let profile = command.profile();
    command.validate_shape()?;
    if deadline.is_zero()
        || !is_pinned_image(&limits.image)
        || limits.pids_limit == 0
        || !safe_nonempty(&limits.memory_limit)
        || !safe_nonempty(&limits.cpu_limit)
        || stdout_limit == 0
        || stderr_limit == 0
    {
        return Err(GitPlanError::Limits);
    }
    if !is_safe_container_name(&container_name) {
        return Err(GitPlanError::Command);
    }
    let combined_limit = stdout_limit
        .checked_add(stderr_limit)
        .ok_or(GitPlanError::Limits)?;
    let scratch = stdout_limit.clamp(4096, 16 * 1024 * 1024);
    let mut args = vec![
        "docker".to_owned(),
        "run".to_owned(),
        "--rm".to_owned(),
        "--pull=never".to_owned(),
        "--name".to_owned(),
        container_name.clone(),
        "--user".to_owned(),
        format!("{}:{}", source.uid(), source.gid()),
        "--read-only".to_owned(),
        "--cap-drop=ALL".to_owned(),
        "--security-opt=no-new-privileges".to_owned(),
        "--memory".to_owned(),
        limits.memory_limit.clone(),
        "--cpus".to_owned(),
        limits.cpu_limit.clone(),
        "--pids-limit".to_owned(),
        limits.pids_limit.to_string(),
        "--tmpfs".to_owned(),
        format!("/tmp:rw,nosuid,nodev,noexec,size={scratch}"),
        "--shm-size".to_owned(),
        format!("{scratch}"),
        "--ulimit".to_owned(),
        format!(
            "fsize={}",
            limits.fetch_volume_bytes.min(u64::from(u32::MAX))
        ),
    ];
    if profile.uses_network() {
        if !is_valid_network(&limits.network) {
            return Err(GitPlanError::Network);
        }
        args.push("--network".to_owned());
        args.push(limits.network.clone());
    } else {
        args.push("--network=none".to_owned());
    }
    for slot in profile.mount_slots() {
        args.push("--mount".to_owned());
        args.push(source.mount(*slot, profile)?);
    }
    for env in fixed_environment(profile) {
        args.push("--env".to_owned());
        args.push(env);
    }
    args.push(limits.image.clone());
    args.extend(command.argv());
    let plan = GitPlan {
        profile,
        command,
        docker_args: args,
        cleanup_args: vec![
            "docker".to_owned(),
            "rm".to_owned(),
            "-f".to_owned(),
            container_name,
        ],
        stdout_limit,
        stderr_limit,
        combined_limit,
        deadline,
    };
    plan.validate()?;
    Ok(plan)
}

fn fixed_environment(profile: GitCloneProfile) -> Vec<String> {
    let home = if profile.install() {
        "/tmp"
    } else {
        CONTROL_TARGET
    };
    let mut env = vec![
        format!("HOME={home}"),
        "GIT_CONFIG_NOSYSTEM=1".to_owned(),
        "GIT_CONFIG_GLOBAL=/dev/null".to_owned(),
        "GIT_TERMINAL_PROMPT=0".to_owned(),
        "HTTP_PROXY=".to_owned(),
        "HTTPS_PROXY=".to_owned(),
        "ALL_PROXY=".to_owned(),
        "NO_PROXY=".to_owned(),
    ];
    if profile.uses_credential() {
        env.push(format!("GIT_ASKPASS={CONTROL_TARGET}/askpass.sh"));
        env.push("GIT_ASKPASS_REQUIRE=force".to_owned());
    }
    env
}

fn is_pinned_image(image: &str) -> bool {
    let Some((_, digest)) = image.rsplit_once("@sha256:") else {
        return false;
    };
    !image.chars().any(|character| {
        character == '\0' || character == '\n' || character == '\r' || character == ' '
    }) && is_lower_hex_exact(digest, 64)
}

fn is_valid_network(network: &str) -> bool {
    !network.is_empty()
        && network.len() <= 128
        && !matches!(
            network.to_ascii_lowercase().as_str(),
            "host" | "none" | "container" | "default" | "bridge"
        )
        && !network.starts_with('-')
        && network
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_valid_broker_host(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 253
        || host.contains("://")
        || host.contains('@')
        || host.contains('/')
        || host.contains('?')
        || host.contains('#')
        || host.contains('*')
        || host.chars().any(|character| character.is_whitespace())
    {
        return false;
    }
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        return false;
    }
    match port {
        Some(port) => port.parse::<u16>().is_ok_and(|port| port > 0),
        None => true,
    }
}

fn is_safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !value.starts_with('.')
}

fn is_safe_container_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn safe_nonempty(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value
            .chars()
            .any(|character| character == '\0' || character == '\n' || character == '\r')
}

fn is_lower_hex_exact(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitPlanOutcome {
    pub(crate) exit_code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitCloneExecutorError {
    TimedOut,
    Cancelled,
    OutputLimit,
    ReapTimeout,
    Failed,
}

pub(crate) type GitPlanFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GitPlanOutcome, GitCloneExecutorError>> + Send + 'a>>;

/// Injected production/test boundary. Every method accepts only a validated
/// closed plan.
pub(crate) trait GitCloneExecutor: Send + Sync {
    fn run_plan<'a>(
        &'a self,
        plan: &'a GitPlan,
        cancel: Option<watch::Receiver<bool>>,
    ) -> GitPlanFuture<'a>;

    fn create_volume<'a>(&'a self, plan: &'a GitVolumePlan) -> GitPlanFuture<'a>;

    fn remove_volume<'a>(&'a self, plan: &'a GitVolumePlan) -> GitPlanFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> GitCloneLimits {
        GitCloneLimits {
            image: format!("helper@sha256:{}", "a".repeat(64)),
            network: "df-git-net".to_owned(),
            broker_host: "broker.example.invalid:443".to_owned(),
            uid: 1000,
            gid: 1000,
            fetch_volume_bytes: 1024 * 1024,
            bundle_bytes: 1024 * 1024,
            logical_object_bytes: 1024 * 1024,
            checkout_bytes: 1024 * 1024,
            tree_entries: 1024,
            deadline: Duration::from_secs(60),
            memory_limit: "256m".to_owned(),
            cpu_limit: "1".to_owned(),
            pids_limit: 64,
        }
    }

    fn paths() -> GitClonePaths {
        GitClonePaths {
            control_dir: PathBuf::from("/tmp/control"),
            credential_dir: PathBuf::from("/tmp/credential"),
            volume_name: "dfgit-abc".to_owned(),
            uid: 1000,
            gid: 1000,
        }
    }

    fn command(profile: GitCloneProfile) -> GitCloneCommand {
        match profile {
            GitCloneProfile::Proof => GitCloneCommand::Proof {
                proof_name: "mount-proof-abc".to_owned(),
            },
            GitCloneProfile::NetLsRemote => GitCloneCommand::NetLsRemote {
                remote_url: "https://broker.example.invalid/api/internal/code-projects/x/git"
                    .to_owned(),
                branch: "main".to_owned(),
            },
            GitCloneProfile::FetchInit => GitCloneCommand::FetchInit {
                object_format: GitObjectFormat::Sha1,
                branch: "main".to_owned(),
            },
            GitCloneProfile::Fetch => GitCloneCommand::Fetch {
                remote_url: "https://broker.example.invalid/api/internal/code-projects/x/git"
                    .to_owned(),
                branch: "main".to_owned(),
            },
            GitCloneProfile::Verify => GitCloneCommand::Verify {
                head: "a".repeat(40),
                expected_base: None,
            },
            GitCloneProfile::InstallClone => GitCloneCommand::InstallRemoveOrigin,
            GitCloneProfile::InstallVerify => GitCloneCommand::InstallStatusClean,
            GitCloneProfile::InstallExport => GitCloneCommand::InstallExport,
            GitCloneProfile::WorkspaceInspect => GitCloneCommand::WorkspaceHead,
            GitCloneProfile::WorkspaceUpdate => GitCloneCommand::WorkspaceImportBundle,
        }
    }

    #[test]
    fn profile_mounts_and_network_are_closed() {
        for profile in [
            GitCloneProfile::Proof,
            GitCloneProfile::NetLsRemote,
            GitCloneProfile::FetchInit,
            GitCloneProfile::Fetch,
            GitCloneProfile::Verify,
        ] {
            let plan = GitPlan::new(
                command(profile),
                &paths(),
                &limits(),
                format!("dfgit-{profile:?}"),
                Duration::from_secs(10),
                1024,
                1024,
            )
            .expect("plan");
            assert_eq!(plan.profile, profile);
            assert_eq!(
                plan.docker_args()
                    .iter()
                    .filter(|argument| *argument == "--mount")
                    .count(),
                profile.mount_slots().len()
            );
            assert_eq!(
                plan.docker_args()
                    .iter()
                    .any(|argument| argument.contains("target=/git-cred")),
                profile.uses_credential()
            );
            assert_eq!(
                plan.docker_args()
                    .iter()
                    .any(|argument| argument == "--network"),
                profile.uses_network()
            );
            if !profile.uses_network() {
                assert!(
                    plan.docker_args()
                        .iter()
                        .any(|argument| argument == "--network=none")
                );
            }
        }
    }

    #[test]
    fn option_injection_and_unsafe_commands_are_rejected() {
        let mut bad = command(GitCloneProfile::Fetch);
        if let GitCloneCommand::Fetch { branch, .. } = &mut bad {
            *branch = "main; rm -rf /".to_owned();
        }
        assert!(
            GitPlan::new(
                bad,
                &paths(),
                &limits(),
                "dfgit-bad".to_owned(),
                Duration::from_secs(10),
                1024,
                1024
            )
            .is_err()
        );
        if let GitCloneCommand::Fetch { remote_url, .. } = &mut command(GitCloneProfile::Fetch) {
            *remote_url = String::new();
        }
        assert!(!is_valid_network("host"));
        assert!(!is_valid_network("-df"));
        assert!(!is_valid_broker_host("user@broker"));
        assert!(is_valid_broker_host("broker.example.invalid:443"));
        assert!(is_pinned_image(&format!(
            "helper@sha256:{}",
            "a".repeat(64)
        )));
        assert!(!is_pinned_image("helper:latest"));
    }
}
