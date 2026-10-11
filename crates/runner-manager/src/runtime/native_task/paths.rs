//! Path derivation and root validation for native-task checkouts
//! (issue 753 P2b).
//!
//! Configured roots are inspected component by component with `symlink_metadata`.
//! Every manager-owned descendant is then checked for its expected uid, type
//! and private mode before it is opened or reused.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::bail;
use uuid::Uuid;

/// Mode a checkout directory must carry.
pub(crate) const TREE_DIR_MODE: u32 = 0o700;
/// Mode a materialized file must carry.
pub(crate) const TREE_FILE_MODE: u32 = 0o600;

/// The resolved directory layout of one task checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskLayout {
    /// `<root>/<environment_uuid>`.
    pub(crate) environment_dir: PathBuf,
    /// `<root>/<environment_uuid>/<task_uuid>`.
    pub(crate) task_dir: PathBuf,
    /// `<task_dir>/tree`, the published checkout.
    pub(crate) tree_dir: PathBuf,
    /// `<task_dir>/.staging`, the unpublished candidate tree.
    pub(crate) staging_dir: PathBuf,
    /// `<task_dir>/journal.json`, the durable identity/phase record.
    pub(crate) journal_path: PathBuf,
}

/// Parse one trusted UUID used as a directory name.
///
/// Both the environment id and the task id are already typed by the durable
/// job; re-parsing here means a malformed value can never become a path
/// component.
pub(crate) fn trusted_uuid_dir(value: &str) -> anyhow::Result<String> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| anyhow::anyhow!("native task path component is not a UUID: {value:?}"))?;
    Ok(parsed.to_string())
}

/// Derive the layout for one environment/task pair under a trusted root.
pub(crate) fn layout(
    root: &Path,
    environment_id: &str,
    task_id: Uuid,
) -> anyhow::Result<TaskLayout> {
    let environment_dir = root.join(trusted_uuid_dir(environment_id)?);
    let task_dir = environment_dir.join(task_id.to_string());
    Ok(TaskLayout {
        tree_dir: task_dir.join("tree"),
        staging_dir: task_dir.join(".staging"),
        journal_path: task_dir.join("journal.json"),
        environment_dir,
        task_dir,
    })
}

/// Validate an absolute configured root and every path component leading to it.
///
/// Ancestors must be real directories, never symlinks. The configured root
/// itself must exist, be owned by `uid`, and have exactly mode 0700. It is
/// never created or repaired.
pub(crate) fn validate_root(root: &Path, uid: u32) -> anyhow::Result<()> {
    if !root.is_absolute() {
        bail!("native task root must be an absolute path");
    }

    let mut current = PathBuf::from("/");
    let mut final_metadata = fs::symlink_metadata(&current)
        .map_err(|error| anyhow::anyhow!("failed to inspect native task root ancestor: {error}"))?;
    for component in root.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            bail!("native task root must not contain relative path components");
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            anyhow::anyhow!(
                "native task root component is unavailable ({}): {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            bail!("native task root chain must not contain symlinks");
        }
        if !metadata.is_dir() {
            bail!("native task root chain must contain only directories");
        }
        final_metadata = metadata;
    }

    if final_metadata.uid() != uid {
        bail!("native task root must be owned by this manager user");
    }
    if final_metadata.permissions().mode() & 0o7777 != TREE_DIR_MODE {
        bail!("native task root must have mode 0700");
    }
    Ok(())
}

/// Create `directory` owner-only when it is absent, then validate it.
///
/// An existing directory is validated as-is: a permissive or foreign directory
/// is refused, never silently tightened.
pub(crate) fn ensure_private_dir(directory: &Path, uid: u32) -> anyhow::Result<()> {
    match fs::symlink_metadata(directory) {
        Ok(_) => validate_private_dir(directory, uid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(directory).map_err(|error| {
                anyhow::anyhow!(
                    "failed to create native task directory {}: {error}",
                    directory.display()
                )
            })?;
            fs::set_permissions(directory, fs::Permissions::from_mode(TREE_DIR_MODE)).map_err(
                |error| {
                    anyhow::anyhow!(
                        "failed to restrict native task directory {}: {error}",
                        directory.display()
                    )
                },
            )?;
            validate_private_dir(directory, uid)
        }
        Err(error) => bail!(
            "failed to inspect native task directory {}: {error}",
            directory.display()
        ),
    }
}

/// Validate that an existing entry is a real manager-owned 0700 directory.
pub(crate) fn validate_private_dir(directory: &Path, uid: u32) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        anyhow::anyhow!(
            "failed to inspect native task directory {}: {error}",
            directory.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "native task directory {} must be a real directory, not a link",
            directory.display()
        );
    }
    if metadata.uid() != uid {
        bail!(
            "native task directory {} must be owned by this manager user",
            directory.display()
        );
    }
    if metadata.permissions().mode() & 0o7777 != TREE_DIR_MODE {
        bail!(
            "native task directory {} must have mode 0700",
            directory.display()
        );
    }
    Ok(())
}

/// Validate a published task tree root after execution may have edited it.
///
/// Replays check only this owned mount root and the journal; task-created
/// descendants are deliberately not compared with the original seed.
pub(crate) fn validate_owned_tree_root(directory: &Path, uid: u32) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| anyhow::anyhow!("failed to inspect the native task tree root: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o7777 != TREE_DIR_MODE
    {
        bail!("native task tree root is not an owned private directory");
    }
    Ok(())
}

/// Refuse a path that is not a regular, manager-owned private file.
///
/// A hard link is also refused: `nlink` greater than one means the inode is
/// reachable from somewhere this manager did not create.
pub(crate) fn refuse_link_or_irregular(path: &Path, uid: u32) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "native task path {} is not available: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        bail!("native task path {} must not be a symlink", path.display());
    }
    if !metadata.is_file() {
        bail!("native task path {} must be a regular file", path.display());
    }
    if metadata.uid() != uid {
        bail!("native task file {} must be manager-owned", path.display());
    }
    if metadata.permissions().mode() & 0o7777 != TREE_FILE_MODE {
        bail!("native task file {} must have mode 0600", path.display());
    }
    if metadata.nlink() != 1 {
        bail!(
            "native task file {} must not be hard-linked",
            path.display()
        );
    }
    Ok(())
}
