//! The per-task journal (issue 753 P2b).
//!
//! The journal records only task identity and phase. `preparing` is retained
//! after interruption; `prepared` is reusable only when the opened file and the
//! published tree still satisfy the manager's ownership and content checks.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::paths::TREE_FILE_MODE;

/// Journal schema version.
pub(crate) const JOURNAL_VERSION: u32 = 1;
/// Largest journal accepted, comfortably above any legitimate record.
const MAX_JOURNAL_BYTES: u64 = 16 * 1024;

/// Which side of the publish boundary a journal records.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JournalPhase {
    /// Staging began; the tree is not published and must not be reused.
    Preparing,
    /// The tree is published and may be replayed as a proof.
    Prepared,
}

/// The durable identity of one task checkout.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct Journal {
    pub(crate) version: u32,
    pub(crate) phase: JournalPhase,
    /// The environment owner identity, from the durable job.
    pub(crate) environment_id: String,
    pub(crate) generation: i64,
    pub(crate) task_id: Uuid,
    pub(crate) workspace_binding_id: i64,
    pub(crate) manifest_sha256: String,
    pub(crate) file_count: u32,
    pub(crate) total_bytes: u32,
}

impl Journal {
    /// True when this journal describes exactly the same materialization as
    /// `other`, ignoring the phase.
    pub(crate) fn same_identity(&self, other: &Journal) -> bool {
        self.version == other.version
            && self.environment_id == other.environment_id
            && self.generation == other.generation
            && self.task_id == other.task_id
            && self.workspace_binding_id == other.workspace_binding_id
            && self.manifest_sha256 == other.manifest_sha256
            && self.file_count == other.file_count
            && self.total_bytes == other.total_bytes
    }
}

/// Write a fresh `preparing` journal, failing when any journal already exists.
pub(crate) fn write_preparing(path: &Path, journal: &Journal, uid: u32) -> anyhow::Result<()> {
    if journal.phase != JournalPhase::Preparing {
        bail!("a new native task journal must begin in the preparing phase");
    }
    let bytes = serde_json::to_vec(journal)
        .map_err(|error| anyhow::anyhow!("failed to encode the native task journal: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(TREE_FILE_MODE)
        .open(path)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => anyhow::anyhow!(
                "a native task journal already exists at {}; refusing to overwrite it",
                path.display()
            ),
            _ => anyhow::anyhow!(
                "failed to create the native task journal {}: {error}",
                path.display()
            ),
        })?;
    validate_journal_metadata(&file.metadata()?, uid)?;
    file.write_all(&bytes)
        .map_err(|error| anyhow::anyhow!("failed to write the native task journal: {error}"))?;
    file.sync_all()
        .map_err(|error| anyhow::anyhow!("failed to sync the native task journal: {error}"))?;
    if file.metadata()?.len() != bytes.len() as u64 {
        bail!("the native task journal was not written completely");
    }
    sync_parent(path)
}

/// Atomically replace the validated `preparing` journal with its `prepared`
/// identity. Unknown, hard-linked or changed journals are never overwritten.
pub(crate) fn write_prepared(path: &Path, journal: &Journal, uid: u32) -> anyhow::Result<()> {
    if journal.phase != JournalPhase::Prepared {
        bail!("a completed native task journal must use the prepared phase");
    }
    let Some(current) = read(path, uid)? else {
        bail!("the preparing native task journal is missing");
    };
    if current.phase != JournalPhase::Preparing || !current.same_identity(journal) {
        bail!("the native task journal changed before publication");
    }

    let bytes = serde_json::to_vec(journal)
        .map_err(|error| anyhow::anyhow!("failed to encode the native task journal: {error}"))?;
    let staging = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(TREE_FILE_MODE)
        .open(&staging)
        .map_err(|error| {
            anyhow::anyhow!("failed to create the native task journal staging file: {error}")
        })?;
    validate_journal_metadata(&file.metadata()?, uid)?;
    file.write_all(&bytes)
        .map_err(|error| anyhow::anyhow!("failed to write the native task journal: {error}"))?;
    file.sync_all()
        .map_err(|error| anyhow::anyhow!("failed to sync the native task journal: {error}"))?;
    if file.metadata()?.len() != bytes.len() as u64 {
        bail!("the staged native task journal was not written completely");
    }
    drop(file);

    let Some(current) = read(path, uid)? else {
        bail!("the preparing native task journal disappeared before publication");
    };
    if current.phase != JournalPhase::Preparing || !current.same_identity(journal) {
        bail!("the native task journal changed before publication");
    }
    fs::rename(&staging, path)
        .map_err(|error| anyhow::anyhow!("failed to publish the native task journal: {error}"))?;
    let Some(published) = read(path, uid)? else {
        bail!("the prepared native task journal is missing after publication");
    };
    if published.phase != JournalPhase::Prepared || !published.same_identity(journal) {
        bail!("the published native task journal does not match its identity");
    }
    sync_parent(path)
}

/// Read an existing journal, if any.
///
/// Both the path entry and the opened file handle must be regular, manager-owned
/// mode-0600 files with one link. A present but invalid journal is an error.
pub(crate) fn read(path: &Path, uid: u32) -> anyhow::Result<Option<Journal>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_journal_metadata(&metadata, uid)?;
            if metadata.len() > MAX_JOURNAL_BYTES {
                bail!("native task journal at {} is oversized", path.display());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            bail!(
                "failed to inspect the native task journal {}: {error}",
                path.display()
            )
        }
    }

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| anyhow::anyhow!("failed to open the native task journal: {error}"))?;
    validate_journal_metadata(&file.metadata()?, uid)?;
    let mut raw = Vec::new();
    file.take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|error| anyhow::anyhow!("failed to read the native task journal: {error}"))?;
    if raw.len() as u64 > MAX_JOURNAL_BYTES {
        bail!("native task journal at {} is oversized", path.display());
    }
    let journal: Journal = serde_json::from_slice(&raw).map_err(|error| {
        anyhow::anyhow!(
            "native task journal at {} is unreadable: {error}",
            path.display()
        )
    })?;
    if journal.version != JOURNAL_VERSION {
        bail!(
            "native task journal at {} has an unsupported version",
            path.display()
        );
    }
    Ok(Some(journal))
}

fn validate_journal_metadata(metadata: &fs::Metadata, uid: u32) -> anyhow::Result<()> {
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o7777 != TREE_FILE_MODE
        || metadata.nlink() != 1
    {
        bail!("native task journal has unsafe type, owner, mode or link count");
    }
    Ok(())
}

/// fsync a file's parent directory so a create or rename is durable.
fn sync_parent(path: &Path) -> anyhow::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)
        .map_err(|error| anyhow::anyhow!("failed to open the native task directory: {error}"))?
        .sync_all()
        .map_err(|error| anyhow::anyhow!("failed to sync the native task directory: {error}"))
}
