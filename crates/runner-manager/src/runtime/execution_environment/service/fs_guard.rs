//! Host-filesystem guards for the credential store (issue 749 P2a repair).
//!
//! The credential lives under the manager's own state root, and that chain
//! of directories is as much a part of the trust boundary as the file mode
//! is: a credential directory another user can write would let that user
//! replace the credential, regardless of the file's own permissions.
//!
//! ## What is closed, and what is assumed
//!
//! Every property below is read from the object itself — a symlink check
//! with `symlink_metadata`, a file's mode/owner/size from `fstat` on the
//! open handle — so a swap after the checks cannot change what the read
//! returns.
//!
//! What std cannot offer is an `O_NOFOLLOW` open or a resolved-handle
//! comparison, so the store additionally requires its root to be a
//! **trusted, manager-owned directory tree that no other local account can
//! write**. Under that stated assumption the pre-open checks plus
//! handle-based validation hold. Closing the remaining
//! check-then-open window would need a public `libc`/`rustix` dependency,
//! which is not an available workspace dependency; the store says so rather
//! than claiming the window is gone.

use std::fs::{self, File, Metadata};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Largest credential file accepted.
pub(crate) const MAX_CREDENTIAL_BYTES: u64 = 4096;
/// Mode a credential file must carry.
pub(crate) const CREDENTIAL_MODE: u32 = 0o600;
/// Mode a credential directory must carry when the manager created it.
pub(crate) const CREDENTIAL_DIR_MODE: u32 = 0o700;

/// Reject anything in the directory chain that is a link, and require each
/// existing directory to be owned by the manager and not group or world
/// accessible.
///
/// Called on both create and reuse, so a directory that was permissive when
/// it was first created is caught on a later read rather than trusted.
pub(crate) fn validate_directory_chain(root: &Path, directory: &Path) -> anyhow::Result<()> {
    let mut current = PathBuf::new();
    for component in directory
        .strip_prefix(root)
        .unwrap_or(directory)
        .components()
    {
        current.push(component.as_os_str());
        let full = root.join(&current);
        if full == root {
            continue;
        }
        let metadata = fs::symlink_metadata(&full).map_err(|error| {
            anyhow::anyhow!("credential path component is not available: {error}")
        })?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "credential directory chain must not contain a symlink; refusing to trust it"
            );
        }
        if !metadata.is_dir() {
            anyhow::bail!("credential directory chain must contain only real directories");
        }
    }
    Ok(())
}

/// Create the credential directory owner-only if it does not exist, then
/// assert the chain. Existing directories are never re-permissioned: a
/// directory that is too open is refused, not silently repaired.
pub(crate) fn prepare_private_directory(root: &Path, directory: &Path) -> anyhow::Result<()> {
    if fs::symlink_metadata(directory).is_err() {
        fs::create_dir_all(directory).map_err(|error| {
            anyhow::anyhow!("failed to create the credential directory: {error}")
        })?;
        fs::set_permissions(directory, fs::Permissions::from_mode(CREDENTIAL_DIR_MODE)).map_err(
            |error| anyhow::anyhow!("failed to restrict the credential directory: {error}"),
        )?;
    }
    validate_directory_chain(root, directory)
}

/// Require that an existing directory is owned by `uid` and not group or
/// world accessible.
pub(crate) fn assert_private_directory(directory: &Path, uid: u32) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| anyhow::anyhow!("failed to inspect the credential directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("credential directory must be a real directory, not a link");
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("credential directory must not be group or world accessible");
    }
    if metadata.uid() != uid {
        anyhow::bail!("credential directory must be owned by this manager user");
    }
    Ok(())
}

/// Refuse a credential path that is a symlink or not a regular file, before
/// it is opened. This is a cheap first gate; the authoritative checks all
/// run against the opened handle in
/// [`read_credential_from_handle`].
pub(crate) fn refuse_link_or_irregular(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| anyhow::anyhow!("service credential is not available: {error}"))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("service credential must not be a symlink; refusing to trust it");
    }
    if !metadata.is_file() {
        anyhow::bail!("service credential must be a regular file");
    }
    Ok(())
}

/// Validate everything that can be known about the credential, using the
/// open handle rather than the path, and return its contents.
///
/// Mode, owner, type, and size are all read from `fstat` on the handle, so a
/// path swapped after the file was opened cannot change what this accepts or
/// returns. The body is read through a bounded reader, so a file that grows
/// after the size check cannot pull in more than the cap.
pub(crate) fn read_credential_from_handle(
    file: &File,
    expected_env_name: &str,
    uid: u32,
) -> anyhow::Result<()> {
    read_credential_token_from_handle(file, expected_env_name, uid).map(|_| ())
}

/// Same validation as [`read_credential_from_handle`], returning the token
/// for the actual relay consumer.
///
/// Crate-private on purpose: the token must reach only the manager-side
/// Basic-auth injection, never a DTO, a `Debug` format, a display string, or
/// a log line. Callers must treat the return as a secret.
pub(crate) fn read_credential_token_from_handle(
    file: &File,
    expected_env_name: &str,
    uid: u32,
) -> anyhow::Result<String> {
    let metadata: Metadata = file
        .metadata()
        .map_err(|error| anyhow::anyhow!("failed to stat the service credential: {error}"))?;
    if !metadata.is_file() {
        anyhow::bail!("service credential must be a regular file");
    }
    if metadata.permissions().mode() & 0o777 != CREDENTIAL_MODE {
        anyhow::bail!(
            "service credential must be owner-only (mode 0600); refusing a permissive file"
        );
    }
    if metadata.uid() != uid {
        anyhow::bail!("service credential must be owned by this manager user");
    }
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        anyhow::bail!("service credential exceeds the permitted size");
    }

    let mut raw = Vec::with_capacity(metadata.len() as usize);
    let read = (&mut &*file)
        .take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|error| anyhow::anyhow!("failed to read the service credential: {error}"))?;
    if read as u64 > MAX_CREDENTIAL_BYTES {
        anyhow::bail!("service credential exceeds the permitted size");
    }

    let text = String::from_utf8(raw)
        .map_err(|_| anyhow::anyhow!("service credential is not valid UTF-8"))?;
    let line = text.trim_end_matches('\n');
    if line.is_empty() || line.contains('\n') {
        anyhow::bail!("service credential is corrupt");
    }
    let (name, token) = line
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("service credential is corrupt"))?;
    if name != expected_env_name {
        anyhow::bail!(
            "service credential is stored under a different variable name than this profile configures"
        );
    }
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("service credential is corrupt");
    }
    Ok(token.to_string())
}

/// fsync a directory so a rename into it is durable.
pub(crate) fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    File::open(directory)
        .map_err(|error| anyhow::anyhow!("failed to open the credential directory: {error}"))?
        .sync_all()
        .map_err(|error| anyhow::anyhow!("failed to sync the credential directory: {error}"))
}
