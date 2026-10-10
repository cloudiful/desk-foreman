//! Component-by-component validation for published native-task trees.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};

use anyhow::bail;
use runner_protocol::SourceSeedSnapshotFile;

use super::super::paths::{TREE_FILE_MODE, refuse_link_or_irregular, validate_private_dir};

/// Validate every tree entry and require the exact file/directory manifest.
pub(super) fn validate_tree_entries(
    tree_dir: &Path,
    files: &[SourceSeedSnapshotFile],
    uid: u32,
) -> anyhow::Result<()> {
    let expected_files: HashSet<String> = files.iter().map(|file| file.path.clone()).collect();
    let mut expected_dirs = HashSet::new();
    for file in files {
        let mut end = 0;
        while let Some(offset) = file.path[end..].find('/') {
            end += offset;
            expected_dirs.insert(file.path[..end].to_string());
            end += 1;
        }
    }
    let mut actual_files = HashSet::new();
    let mut actual_dirs = HashSet::new();
    validate_entries(
        tree_dir,
        tree_dir,
        uid,
        &expected_files,
        &expected_dirs,
        &mut actual_files,
        &mut actual_dirs,
    )?;
    if actual_files != expected_files || actual_dirs != expected_dirs {
        bail!("the published native task tree does not match its manifest paths");
    }
    Ok(())
}

fn validate_entries(
    root: &Path,
    directory: &Path,
    uid: u32,
    expected_files: &HashSet<String>,
    expected_dirs: &HashSet<String>,
    actual_files: &mut HashSet<String>,
    actual_dirs: &mut HashSet<String>,
) -> anyhow::Result<()> {
    validate_private_dir(directory, uid)?;
    for entry in fs::read_dir(directory).map_err(|error| {
        anyhow::anyhow!("failed to read the published native task tree: {error}")
    })? {
        let path = entry
            .map_err(|error| {
                anyhow::anyhow!("failed to read the published native task tree: {error}")
            })?
            .path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            anyhow::anyhow!("failed to inspect a published native task entry: {error}")
        })?;
        if metadata.file_type().is_symlink() {
            bail!("published native task tree entries must not be symlinks");
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| anyhow::anyhow!("native task path escaped its tree: {error}"))?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("native task path is not valid UTF-8"))?;
        if metadata.is_dir() {
            if !expected_dirs.contains(relative) {
                bail!("the published native task tree contains an unexpected directory");
            }
            actual_dirs.insert(relative.to_string());
            validate_private_dir(&path, uid)?;
            validate_entries(
                root,
                &path,
                uid,
                expected_files,
                expected_dirs,
                actual_files,
                actual_dirs,
            )?;
        } else if metadata.is_file() {
            if !expected_files.contains(relative) {
                bail!("the published native task tree contains an unexpected file");
            }
            refuse_link_or_irregular(&path, uid)?;
            let file = OpenOptions::new().read(true).open(&path).map_err(|error| {
                anyhow::anyhow!("failed to open a published native task file: {error}")
            })?;
            validate_file_metadata(&file.metadata()?, uid)?;
            actual_files.insert(relative.to_string());
        } else {
            bail!("published native task tree entries must be regular files or directories");
        }
    }
    Ok(())
}

/// Validate a traversed directory before looking up a file beneath it.
pub(super) fn validate_parent_directories(
    tree_dir: &Path,
    relative_file: &str,
    uid: u32,
) -> anyhow::Result<()> {
    validate_private_dir(tree_dir, uid)?;
    let components: Vec<_> = Path::new(relative_file).components().collect();
    let mut current = tree_dir.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let Component::Normal(name) = component else {
            bail!("native task file path is not a normalized relative path");
        };
        current.push(name);
        validate_private_dir(&current, uid)?;
    }
    Ok(())
}

/// Read a regular file after validating its path entry and opened handle.
pub(super) fn read_file(path: &Path, uid: u32) -> anyhow::Result<Vec<u8>> {
    refuse_link_or_irregular(path, uid)?;
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| anyhow::anyhow!("failed to open a native task file: {error}"))?;
    validate_file_metadata(&file.metadata()?, uid)?;
    let mut content = Vec::new();
    file.take(runner_protocol::MAX_SEED_CONTENT_BYTES as u64 + 1)
        .read_to_end(&mut content)
        .map_err(|error| anyhow::anyhow!("failed to read a native task file: {error}"))?;
    if content.len() > runner_protocol::MAX_SEED_CONTENT_BYTES {
        bail!("a native task file exceeds the permitted size");
    }
    Ok(content)
}

pub(super) fn validate_file_metadata(metadata: &fs::Metadata, uid: u32) -> anyhow::Result<()> {
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o7777 != TREE_FILE_MODE
        || metadata.nlink() != 1
    {
        bail!("native task file handle has unsafe type, owner, mode or link count");
    }
    Ok(())
}

/// fsync a validated tree file.
pub(super) fn sync_file(file: &File, uid: u32) -> anyhow::Result<()> {
    validate_file_metadata(&file.metadata()?, uid)?;
    file.sync_all()
        .map_err(|error| anyhow::anyhow!("failed to sync a native task file: {error}"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::validate_file_metadata;
    use crate::runtime::execution_environment::service::host_identity::UidSource;

    #[test]
    fn file_owner_is_checked_with_an_injected_expected_uid() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("input.txt");
        std::fs::write(&path, b"input").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("file mode");
        let metadata = std::fs::symlink_metadata(&path).expect("metadata");
        let uid = UidSource::Proc.effective_uid().expect("effective uid");
        assert!(validate_file_metadata(&metadata, uid + 1).is_err());
    }
}
