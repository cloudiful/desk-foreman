//! Staging, verification and publishing of one checkout tree
//! (issue 753 P2b).
//!
//! The tree is written into a private `.staging` directory that is a sibling of
//! the final `tree`, then renamed into place. Nothing is ever written directly
//! into a published tree, every component is created with `create_new` so an
//! existing path is never overwritten, and each written file is validated
//! against the open handle: regular, owner-only, owned by this manager, and
//! exactly the expected length.
//!
//! After the copy the manifest is recomputed from the bytes on disk and
//! compared with the digest the control plane sealed, so a truncated or
//! corrupted write can never be published.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path};

use anyhow::bail;
use runner_protocol::SourceSeedSnapshotFile;

mod validation;

use self::validation::{
    read_file, validate_file_metadata, validate_parent_directories, validate_tree_entries,
};
use super::paths::{TREE_DIR_MODE, TREE_FILE_MODE, refuse_link_or_irregular, validate_private_dir};

/// Stage every file of `files` (already in canonical path order) into
/// `staging_dir`, verify the recomputed manifest, then publish by renaming the
/// staging directory to `tree_dir`.
pub(crate) fn stage_and_publish(
    staging_dir: &Path,
    tree_dir: &Path,
    files: &[SourceSeedSnapshotFile],
    expected_manifest: &str,
    uid: u32,
) -> anyhow::Result<()> {
    if fs::symlink_metadata(tree_dir).is_ok() {
        bail!(
            "a native task tree already exists at {}; refusing to overwrite it",
            tree_dir.display()
        );
    }
    if fs::symlink_metadata(staging_dir).is_ok() {
        bail!(
            "a native task staging directory already exists at {}; refusing to reuse it",
            staging_dir.display()
        );
    }

    fs::create_dir(staging_dir).map_err(|error| {
        anyhow::anyhow!("failed to create the native task staging directory: {error}")
    })?;
    fs::set_permissions(staging_dir, fs::Permissions::from_mode(TREE_DIR_MODE)).map_err(
        |error| anyhow::anyhow!("failed to restrict the native task staging directory: {error}"),
    )?;
    validate_private_dir(staging_dir, uid)?;

    for file in files {
        let target = staging_dir.join(&file.path);
        if let Some(parent) = target.parent() {
            create_chain(staging_dir, parent, uid)?;
        }
        write_new_file(&target, file.content.as_bytes(), uid)?;
    }

    // The bytes on disk are re-hashed before anything is published.
    let on_disk = read_back(staging_dir, files, uid)?;
    validate_tree_entries(staging_dir, files, uid)?;
    let recomputed = runner_protocol::manifest_sha256(&on_disk);
    if recomputed != expected_manifest {
        bail!("the staged native task tree does not match the sealed manifest");
    }

    sync_tree(staging_dir, uid)?;
    fs::rename(staging_dir, tree_dir)
        .map_err(|error| anyhow::anyhow!("failed to publish the native task tree: {error}"))?;
    validate_private_dir(tree_dir, uid)?;
    sync_dir(tree_dir.parent().unwrap_or(tree_dir))?;
    Ok(())
}

/// Validate a published tree against the sealed manifest and ownership.
///
/// Used on a replay: the tree must already match exactly, and it is not
/// rewritten.
pub(crate) fn validate_published_tree(
    tree_dir: &Path,
    files: &[SourceSeedSnapshotFile],
    expected_manifest: &str,
    uid: u32,
) -> anyhow::Result<()> {
    validate_private_dir(tree_dir, uid)?;
    validate_tree_entries(tree_dir, files, uid)?;
    let on_disk = read_back(tree_dir, files, uid)?;
    let recomputed = runner_protocol::manifest_sha256(&on_disk);
    if recomputed != expected_manifest {
        bail!("the published native task tree does not match the sealed manifest");
    }
    Ok(())
}

/// Re-read the tree and rebuild the manifest file list.
fn read_back(
    tree_dir: &Path,
    files: &[SourceSeedSnapshotFile],
    uid: u32,
) -> anyhow::Result<Vec<SourceSeedSnapshotFile>> {
    let mut verified = Vec::with_capacity(files.len());
    let mut total: u64 = 0;
    for file in files {
        validate_parent_directories(tree_dir, &file.path, uid)?;
        let path = tree_dir.join(&file.path);
        let content = read_file(&path, uid)?;
        if content != file.content.as_bytes() {
            bail!("a native task file does not match the sealed content");
        }
        total = total.saturating_add(content.len() as u64);
        verified.push(SourceSeedSnapshotFile::new(
            file.path.clone(),
            file.content.clone(),
        ));
    }
    if total > runner_protocol::MAX_SEED_TOTAL_BYTES as u64 {
        bail!("the native task tree exceeds the permitted total size");
    }
    Ok(verified)
}

/// Create each missing directory of `target` under `base`, owner-only.
fn create_chain(base: &Path, target: &Path, uid: u32) -> anyhow::Result<()> {
    validate_private_dir(base, uid)?;
    let relative = target.strip_prefix(base).unwrap_or(target);
    let mut current = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            bail!("native task file path is not a normalized relative path");
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(_) => validate_private_dir(&current, uid)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    anyhow::anyhow!("failed to create a native task directory component: {error}")
                })?;
                fs::set_permissions(&current, fs::Permissions::from_mode(TREE_DIR_MODE)).map_err(
                    |error| anyhow::anyhow!("failed to restrict a native task directory: {error}"),
                )?;
                validate_private_dir(&current, uid)?;
            }
            Err(error) => bail!("failed to inspect a native task directory component: {error}"),
        }
    }
    Ok(())
}

/// Write a fresh owner-only file, refusing to overwrite anything.
fn write_new_file(path: &Path, content: &[u8], uid: u32) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(TREE_FILE_MODE)
        .open(path)
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to create a native task file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(content)
        .map_err(|error| anyhow::anyhow!("failed to write a native task file: {error}"))?;
    file.sync_all()
        .map_err(|error| anyhow::anyhow!("failed to sync a native task file: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| anyhow::anyhow!("failed to stat a native task file: {error}"))?;
    validate_file_metadata(&metadata, uid)?;
    if metadata.len() != content.len() as u64 {
        bail!("a native task file was not written as expected");
    }
    Ok(())
}

/// fsync every validated file and directory of a staged tree.
fn sync_tree(dir: &Path, uid: u32) -> anyhow::Result<()> {
    validate_private_dir(dir, uid)?;
    for entry in fs::read_dir(dir)
        .map_err(|error| anyhow::anyhow!("failed to read the staged tree: {error}"))?
    {
        let entry =
            entry.map_err(|error| anyhow::anyhow!("failed to read the staged tree: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| anyhow::anyhow!("failed to inspect the staged tree: {error}"))?;
        if metadata.file_type().is_symlink() {
            bail!("native task tree components must not be symlinks");
        }
        if metadata.is_dir() {
            validate_private_dir(&path, uid)?;
            sync_tree(&path, uid)?;
        } else {
            refuse_link_or_irregular(&path, uid)?;
            let file = File::open(&path)
                .map_err(|error| anyhow::anyhow!("failed to open a native task file: {error}"))?;
            validation::sync_file(&file, uid)?;
        }
    }
    sync_dir(dir)
}

/// fsync one directory.
fn sync_dir(dir: &Path) -> anyhow::Result<()> {
    File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|error| anyhow::anyhow!("failed to sync a native task directory: {error}"))
}
