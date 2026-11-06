//! Scoped private-service workspace derivation and validation
//! (issue 1180 P18R3).
//!
//! The stored environment scope names one workspace binding and a bounded
//! relative path. Everything else — local root, host mount source, state
//! root, image and service profile — is derived here from trusted manager
//! configuration and validated before any filesystem or container mutation.
//! No caller-supplied host path, image, volume or environment entry is
//! accepted, and no path is normalised into a different target.

use std::path::{Component, Path, PathBuf};

use anyhow::bail;
use runner_protocol::execution_environment::ExecutionEnvironmentWorkspaceScope;

use crate::config::RunnerManagerConfig;
use crate::runtime::docker::DockerRunnerBackend;
use crate::runtime::execution_environment::ExecutionEnvironmentProfile;

/// A fully validated scoped private-service launch plan.
pub(crate) struct ScopedServicePlan {
    pub(crate) workspace_binding_id: i64,
    pub(crate) local_root: PathBuf,
    pub(crate) workspace_mount: PathBuf,
    pub(crate) workspace_mount_target: String,
    pub(crate) state_mount_target: String,
    pub(crate) image: String,
    pub(crate) user_spec: String,
}

/// Derive and validate one scoped plan from the held config snapshot.
pub(crate) fn derive_scoped_service_plan(
    config: &RunnerManagerConfig,
    profile: &ExecutionEnvironmentProfile,
    scope: &ExecutionEnvironmentWorkspaceScope,
) -> anyhow::Result<ScopedServicePlan> {
    scope.validate().map_err(anyhow::Error::msg)?;
    let local_root = scoped_local_root(config, scope)?;
    let workspace_mount = DockerRunnerBackend::workspace_mount(config, &local_root)?;
    let user_spec = crate::runtime::docker::workspace_owner_user_spec(&local_root)?;

    let state_dir = profile.ensure_state_dir()?;
    let state_mount_target = profile.state_mount_target.clone();
    validate_container_target(&state_mount_target, "state mount target")?;
    validate_container_target(&config.workdir, "container workdir")?;
    if component_overlap(Path::new(&config.workdir), Path::new(&state_mount_target)) {
        bail!("container workdir and state mount target must be disjoint");
    }

    let canonical_state = state_dir
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("scoped state directory is unavailable"))?;
    let canonical_workspace_root = config
        .workspace_root
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("trusted workspace root is unavailable"))?;
    let canonical_local_root = local_root
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("scoped workspace root is unavailable"))?;
    if overlap(&canonical_state, &canonical_workspace_root)
        || overlap(&canonical_state, &canonical_local_root)
    {
        bail!("scoped state directory overlaps a workspace root");
    }
    for private_root in [config.native_task_root.as_deref()].into_iter().flatten() {
        let canonical = private_root
            .canonicalize()
            .map_err(|_| anyhow::anyhow!("configured private root is unverifiable"))?;
        if overlap(&canonical, &canonical_workspace_root)
            || overlap(&canonical, &canonical_local_root)
        {
            bail!("configured private root overlaps a workspace root");
        }
    }

    let host_workspace_root = normal_absolute_lexical(&config.host_workspace_root)?;
    if !is_normal_absolute(&workspace_mount) {
        bail!("derived workspace mount source is not a normal absolute path");
    }
    for host_private in [config.native_task_host_root.as_deref()]
        .into_iter()
        .flatten()
    {
        let host_private = normal_absolute_lexical(host_private)?;
        if overlap(&host_private, &host_workspace_root) || overlap(&host_private, &workspace_mount)
        {
            bail!("configured host private root overlaps the workspace mount");
        }
    }

    let image = crate::runtime::workspace_service::require_workspace_service_image()?;
    Ok(ScopedServicePlan {
        workspace_binding_id: scope.workspace_binding_id,
        local_root,
        workspace_mount,
        workspace_mount_target: config.workdir.clone(),
        state_mount_target,
        image,
        user_spec,
    })
}

/// The strict, non-symlinked local root for one scope.
fn scoped_local_root(
    config: &RunnerManagerConfig,
    scope: &ExecutionEnvironmentWorkspaceScope,
) -> anyhow::Result<PathBuf> {
    if !config.workspace_root.is_absolute() {
        bail!("trusted workspace root must be absolute");
    }
    let mut root = config.workspace_root.clone();
    let mut components = 0;
    for component in Path::new(&scope.workspace_relative_path).components() {
        let Component::Normal(name) = component else {
            bail!("workspace scope path must contain only normal components");
        };
        let next = root.join(name);
        let metadata = std::fs::symlink_metadata(&next)
            .map_err(|_| anyhow::anyhow!("scoped workspace root component is unavailable"))?;
        if metadata.file_type().is_symlink() {
            bail!("scoped workspace root may not traverse a symbolic link");
        }
        root = next;
        components += 1;
    }
    if components == 0 {
        bail!("workspace scope path must name a strict descendant");
    }
    let metadata = std::fs::metadata(&root)
        .map_err(|_| anyhow::anyhow!("scoped workspace root is unavailable"))?;
    if !metadata.is_dir() {
        bail!("scoped workspace root is not a directory");
    }
    Ok(root)
}

/// Container workdir and state target must be normal absolute non-root paths
/// without mount delimiters or control characters.
fn validate_container_target(value: &str, what: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte == b',' || byte == 0 || byte.is_ascii_control())
    {
        bail!("{what} contains a forbidden character");
    }
    let path = Path::new(value);
    if !path.is_absolute() || path == Path::new("/") {
        bail!("{what} must be a normalized absolute non-root path");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        bail!("{what} must be normalized");
    }
    Ok(())
}

/// Reserved execution-environment environment names a service auth variable
/// must never shadow.
pub(crate) fn is_reserved_service_env(value: &str) -> bool {
    matches!(
        value,
        "DF_STATE_DIRECTORY" | "DF_EXECUTION_ENVIRONMENT" | "DF_GENERATION"
    )
}

fn normal_absolute_lexical(path: &Path) -> anyhow::Result<PathBuf> {
    if !is_normal_absolute(path) {
        bail!("configured host root must be a normal absolute path");
    }
    Ok(path.to_path_buf())
}

fn is_normal_absolute(path: &Path) -> bool {
    if !path.is_absolute() || path == Path::new("/") {
        return false;
    }
    let Some(text) = path.to_str() else {
        return false;
    };
    !text.is_empty()
        && !text
            .bytes()
            .any(|byte| byte == b',' || byte == 0 || byte.is_ascii_control())
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

fn overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn component_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod workspace_scope_tests {
    use std::path::Path;

    use super::{component_overlap, is_normal_absolute, is_reserved_service_env};

    #[test]
    fn service_env_reserves_execution_environment_names() {
        for name in [
            "DF_STATE_DIRECTORY",
            "DF_EXECUTION_ENVIRONMENT",
            "DF_GENERATION",
        ] {
            assert!(is_reserved_service_env(name), "{name}");
        }
        assert!(!is_reserved_service_env("DF_SERVICE_TOKEN"));
    }

    #[test]
    fn container_targets_must_be_normal_absolute_nonroot() {
        assert!(is_normal_absolute(Path::new("/workspace")));
        assert!(!is_normal_absolute(Path::new("workspace")));
        assert!(!is_normal_absolute(Path::new("/")));
        assert!(!is_normal_absolute(Path::new("/workspace/../etc")));
        assert!(!is_normal_absolute(Path::new("/workspace,etc")));
    }

    #[test]
    fn mount_target_overlap_uses_component_boundaries() {
        assert!(component_overlap(
            Path::new("/workspace"),
            Path::new("/workspace")
        ));
        assert!(component_overlap(
            Path::new("/workspace"),
            Path::new("/workspace/state")
        ));
        assert!(!component_overlap(
            Path::new("/workspace"),
            Path::new("/workspace-state")
        ));
    }
}
