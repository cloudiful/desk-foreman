//! Identity-bound ordinary-container quiescence for one workspace binding.
//!
//! This source is inert: nothing calls it from ensure yet. It proves positive
//! absence of every own ordinary container for a binding, refuses rather than
//! stopping any foreign, private, unknown or unverifiable container, and never
//! treats a scan as proof of helper or service quiescence.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::bail;
use runner_protocol::RunnerOwner;

use super::{
    backend::ExpectedOwnContainer,
    docker::DockerRunnerBackend,
    docker_lifecycle::{ContainerMetadata, ContainerMountInfo, owner_from_stable_key},
};

const QUIESCENCE_BUDGET: Duration = Duration::from_secs(60);

enum MountRole {
    Ignore,
    PotentialWriter(PathBuf),
    Unverifiable,
}

struct Candidate {
    owner: RunnerOwner,
    name: String,
    container_id: String,
}

impl DockerRunnerBackend {
    /// Quiesce with an already-held config snapshot.
    ///
    /// The scoped ensure path holds one config read guard for the whole
    /// operation, so it must pass that snapshot instead of reacquiring the
    /// backend lock (which could wait behind a queued writer).
    pub(super) async fn quiesce_workspace_binding_owners_with_config(
        &self,
        config: &crate::config::RunnerManagerConfig,
        workspace_binding_id: i64,
        authorized_local_workspace_root: PathBuf,
        expected_own: Option<ExpectedOwnContainer>,
    ) -> anyhow::Result<()> {
        if workspace_binding_id <= 0 {
            bail!("invalid workspace binding id");
        }
        let expected =
            DockerRunnerBackend::workspace_mount(config, &authorized_local_workspace_root)?;
        if !is_normal_absolute(&expected) {
            bail!("expected workspace source is not a normal absolute path");
        }
        let workdir = config.workdir.clone();
        let expected_own = expected_own.as_ref();
        let operation = async {
            let candidates = self
                .scan_binding_containers(workspace_binding_id, &workdir, &expected, expected_own)
                .await?;
            for candidate in candidates {
                self.stop_verified_candidate(&candidate, &workdir, &expected)
                    .await?;
            }
            let remaining = self
                .scan_binding_containers(workspace_binding_id, &workdir, &expected, expected_own)
                .await?;
            if !remaining.is_empty() {
                bail!("workspace binding container absence was not confirmed");
            }
            Ok(())
        };
        match tokio::time::timeout(QUIESCENCE_BUDGET, operation).await {
            Ok(result) => result,
            Err(_) => bail!("workspace binding quiescence timed out"),
        }
    }

    async fn scan_binding_containers(
        &self,
        workspace_binding_id: i64,
        workdir: &str,
        expected: &Path,
        expected_own: Option<&ExpectedOwnContainer>,
    ) -> anyhow::Result<Vec<Candidate>> {
        let mut names = BTreeSet::new();
        for filter in [
            "label=desk-foreman.managed".to_string(),
            "label=desk-foreman.owner".to_string(),
            "name=desk-foreman-runner-workspace-binding-".to_string(),
            "name=desk-foreman-runner-application_principal-".to_string(),
        ] {
            let listed = self
                .docker_output(["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"])
                .await?;
            names.extend(
                listed
                    .lines()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            );
        }

        let mut candidates = Vec::new();
        for name in names {
            let metadata = self
                .inspect_container_metadata_with_workdir(&name, workdir)
                .await?
                .ok_or_else(|| anyhow::anyhow!("listed container disappeared during inspection"))?;
            let parsed_owner = metadata
                .owner_key
                .as_deref()
                .and_then(owner_from_stable_key);
            let named_owner = named_owner(&name);
            let target_owner = RunnerOwner::WorkspaceBinding {
                workspace_binding_id,
            };
            let canonical_target = target_owner.container_name();

            if let Some(expected_own) = expected_own {
                if metadata.owner_key.as_deref() == Some(expected_own.owner.stable_key().as_str()) {
                    let exact = name == expected_own.name
                        && metadata.container_id.as_deref()
                            == Some(expected_own.container_id.as_str())
                        && metadata.matches_owner(
                            &expected_own.owner,
                            &expected_own.name,
                            &self.manager_id,
                        );
                    if exact {
                        continue;
                    }
                    bail!("expected own service container identity does not match");
                }
            }

            if name == canonical_target && parsed_owner.as_ref() != Some(&target_owner) {
                bail!("workspace binding container identity is unverifiable");
            }
            if let Some(named) = &named_owner {
                if named.workspace_binding_id() == Some(workspace_binding_id)
                    && parsed_owner.as_ref() != Some(named)
                {
                    bail!("workspace binding container identity is unverifiable");
                }
            }

            let candidate_owner = parsed_owner.filter(|owner| {
                owner.workspace_binding_id() == Some(workspace_binding_id)
                    && matches!(
                        owner,
                        RunnerOwner::WorkspaceBinding { .. }
                            | RunnerOwner::ApplicationPrincipal { .. }
                    )
            });
            if let Some(owner) = candidate_owner {
                if !metadata.matches_owner(&owner, &name, &self.manager_id) {
                    bail!("workspace binding container identity is unverifiable");
                }
                if !candidate_layout(&metadata, workdir, expected) {
                    bail!("workspace binding container layout is incompatible");
                }
                let container_id = metadata
                    .container_id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("workspace binding container id is unverifiable")
                    })?;
                candidates.push(Candidate {
                    owner,
                    name,
                    container_id,
                });
                continue;
            }

            for mount in metadata.mounts.as_deref().unwrap_or_default() {
                match classify_mount(mount) {
                    MountRole::Ignore => {}
                    MountRole::PotentialWriter(source) => {
                        if path_overlaps(expected, &source) {
                            bail!("workspace binding is overlapped by another container");
                        }
                    }
                    MountRole::Unverifiable => {
                        bail!("workspace binding has an unverifiable container mount");
                    }
                }
            }
            if metadata.mounts.is_none() {
                bail!("workspace binding has an unverifiable container mount");
            }
        }
        Ok(candidates)
    }

    async fn stop_verified_candidate(
        &self,
        candidate: &Candidate,
        workdir: &str,
        expected: &Path,
    ) -> anyhow::Result<()> {
        let lock = self.runner_lock(&candidate.owner);
        let _guard = lock.lock().await;
        if self.has_active_operation(&candidate.owner.stable_key()) {
            bail!("workspace binding has active operations");
        }
        let fresh = self
            .inspect_container_metadata_with_workdir(&candidate.name, workdir)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("workspace binding container disappeared before stop")
            })?;
        if fresh.container_id.as_deref() != Some(candidate.container_id.as_str())
            || !fresh.matches_owner(&candidate.owner, &candidate.name, &self.manager_id)
            || !candidate_layout(&fresh, workdir, expected)
        {
            bail!("workspace binding container identity changed before stop");
        }
        self.terminate_container_for_takeover(&candidate.container_id)
            .await?;
        if self
            .inspect_container_metadata_with_workdir(&candidate.container_id, workdir)
            .await?
            .is_some()
            || self
                .inspect_container_metadata_with_workdir(&candidate.name, workdir)
                .await?
                .is_some()
        {
            bail!("workspace binding container absence was not confirmed");
        }
        self.report_removed_metadata(&fresh, &candidate.name);
        Ok(())
    }
}

fn candidate_layout(metadata: &ContainerMetadata, workdir: &str, expected: &Path) -> bool {
    if metadata.workspace_source.as_deref() != Some(expected) {
        return false;
    }
    let Some(mounts) = metadata.mounts.as_ref() else {
        return false;
    };
    let workspace_mounts = mounts
        .iter()
        .filter(|mount| mount.destination.as_deref() == Some(workdir))
        .collect::<Vec<_>>();
    if workspace_mounts.len() != 1 {
        return false;
    }
    let workspace = workspace_mounts[0];
    if workspace.kind.as_deref() != Some("bind")
        || workspace.source.as_deref() != Some(expected)
        || workspace.read_write != Some(true)
    {
        return false;
    }
    mounts
        .iter()
        .filter(|mount| mount.destination.as_deref() != Some(workdir))
        .all(|mount| matches!(classify_mount(mount), MountRole::Ignore))
}

fn classify_mount(mount: &ContainerMountInfo) -> MountRole {
    match mount.kind.as_deref() {
        Some("tmpfs") => MountRole::Ignore,
        Some("bind") | Some("volume") => match mount.source.as_deref() {
            _ if mount.read_write == Some(false) => MountRole::Ignore,
            Some(source) if is_normal_absolute(source) => {
                MountRole::PotentialWriter(source.to_path_buf())
            }
            _ => MountRole::Unverifiable,
        },
        _ => MountRole::Unverifiable,
    }
}

fn is_normal_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Some(text) = path.to_str() else {
        return false;
    };
    if text.is_empty()
        || text
            .bytes()
            .any(|byte| byte == b',' || byte == 0 || byte.is_ascii_control())
    {
        return false;
    }
    path.components()
        .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

fn path_overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn named_owner(name: &str) -> Option<RunnerOwner> {
    if let Some(rest) = name.strip_prefix("desk-foreman-runner-workspace-binding-") {
        let id: i64 = rest.parse().ok()?;
        return (id > 0).then_some(RunnerOwner::WorkspaceBinding {
            workspace_binding_id: id,
        });
    }
    let rest = name.strip_prefix("desk-foreman-runner-application_principal-")?;
    let mut parts = rest.split('-');
    let (application_id, workspace_binding_id, digest) =
        (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    owner_from_stable_key(&format!(
        "application_principal:{application_id}:{workspace_binding_id}:{digest}"
    ))
}

#[cfg(test)]
mod path_tests {
    use super::path_overlaps;

    #[test]
    fn path_overlap_uses_component_boundaries() {
        assert!(path_overlaps(
            std::path::Path::new("/host/base/workspace"),
            std::path::Path::new("/host/base/workspace")
        ));
        assert!(path_overlaps(
            std::path::Path::new("/host/base/workspace"),
            std::path::Path::new("/host/base")
        ));
        assert!(path_overlaps(
            std::path::Path::new("/host/base/workspace"),
            std::path::Path::new("/host/base/workspace/child")
        ));
        assert!(!path_overlaps(
            std::path::Path::new("/host/base/workspace"),
            std::path::Path::new("/host/base/workspace-other")
        ));
        assert!(!path_overlaps(
            std::path::Path::new("/host/base/workspace"),
            std::path::Path::new("/host/other")
        ));
    }
}
