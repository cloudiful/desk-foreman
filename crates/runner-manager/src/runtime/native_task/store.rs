//! The native-task materializer (issue 753 P2b).
//!
//! One store per manager. It owns the optional trusted root, the effective-uid
//! source, and a per-task serial mutex so two durable preparations for the same
//! task can never both publish. It is filesystem only: no container, no port,
//! no agent, no canonical project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::OwnedMutexGuard;

use anyhow::bail;
use runner_protocol::{NativeTaskPreparedResult, PrepareNativeTaskRequest, SeedBundle};

use super::journal::{self, Journal, JournalPhase};
use super::paths::{self, TaskLayout};
use super::tree;
use crate::config::RunnerManagerConfig;
use crate::runtime::execution_environment::service::host_identity::UidSource;

/// Fixed diagnostic when preparation is not configured.
///
/// Deliberately constant and secret-free: the control plane maps any failed
/// preparation to an `uncertain` task, so the message only has to be stable
/// and non-disclosing.
const UNSUPPORTED: &str = "native task preparation is not configured on this runner manager";

/// Materializes native-task checkouts under one trusted root.
pub(crate) struct NativeTaskStore {
    root: Option<PathBuf>,
    uid_source: UidSource,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// A task workspace held under the store's per-task mutex.
///
/// The filesystem layout stays private to this backend module; callers can
/// inspect or transition the journal only through the checked operations below.
pub(crate) struct LockedNativeTaskWorkspace {
    layout: TaskLayout,
    task_dir_present: bool,
    _guard: OwnedMutexGuard<()>,
}

impl NativeTaskStore {
    /// Build the store from manager configuration.
    pub(crate) fn from_config(config: &RunnerManagerConfig) -> Self {
        Self {
            root: config.native_task_root.clone(),
            uid_source: UidSource::Proc,
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Build a store with an explicit root and uid source (test only).
    ///
    /// The uid source is only reachable through this type, so a test can
    /// simulate an unavailable or different owner without any way to weaken
    /// the production path.
    #[cfg(test)]
    pub(crate) fn with_uid_source(root: Option<PathBuf>, uid_source: UidSource) -> Self {
        Self {
            root,
            uid_source,
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Materialize (or idempotently replay) one task checkout.
    pub(crate) async fn materialize(
        &self,
        request: &PrepareNativeTaskRequest,
        seed: &SeedBundle,
    ) -> anyhow::Result<NativeTaskPreparedResult> {
        let Some(root) = self.root.as_deref() else {
            bail!(UNSUPPORTED);
        };

        // Reapply the shared path, size, ordering and digest contract before a
        // path reaches the filesystem or any content is written.
        let seed = verify_bundle(seed, request)?;

        let uid = self.uid_source.effective_uid()?;
        if uid == 0 {
            bail!("native task preparation refuses to run as root");
        }
        paths::validate_root(root, uid)?;

        let (environment_id, generation) =
            runner_protocol::execution_environment::as_execution_environment(&request.owner)
                .ok_or_else(|| {
                    anyhow::anyhow!("native task preparation requires an environment owner")
                })?;
        let layout = paths::layout(root, environment_id, request.task_id)?;
        let desired = Journal {
            version: journal::JOURNAL_VERSION,
            phase: JournalPhase::Prepared,
            environment_id: environment_id.to_string(),
            generation,
            task_id: request.task_id,
            workspace_binding_id: request.workspace_binding_id,
            manifest_sha256: request.manifest_sha256.clone(),
            file_count: request.file_count,
            total_bytes: request.total_bytes,
        };

        // One publication per task: a second concurrent preparation for the
        // same task waits here and then observes the prepared journal.
        let lock = self.lock_for(&layout.task_dir);
        let _guard = lock.lock().await;

        paths::ensure_private_dir(&layout.environment_dir, uid)?;
        paths::ensure_private_dir(&layout.task_dir, uid)?;

        match journal::read(&layout.journal_path, uid)? {
            Some(existing) if existing.phase == JournalPhase::Prepared => {
                if !existing.same_identity(&desired) {
                    bail!("the native task journal describes a different materialization");
                }
                // A replay validates the published tree and returns the proof
                // without rewriting anything.
                tree::validate_published_tree(
                    &layout.tree_dir,
                    &seed.files,
                    &request.manifest_sha256,
                    uid,
                )?;
                return Ok(proof(request));
            }
            Some(existing) if existing.phase == JournalPhase::Preparing => {
                // Never wipe and never start a fresh checkout: an interrupted
                // preparation needs explicit reconciliation.
                bail!(
                    "an interrupted native task preparation is retained at {} and requires explicit reconciliation",
                    layout.task_dir.display()
                );
            }
            Some(existing)
                if matches!(
                    existing.phase,
                    JournalPhase::Starting | JournalPhase::Running
                ) =>
            {
                bail!("a native task checkout cannot be rematerialized after execution began")
            }
            Some(_) => bail!("the native task journal has an unknown phase"),
            None => {}
        }

        // Persist the preparing journal before any file content so a crash is
        // always attributable.
        let mut preparing = desired.clone();
        preparing.phase = JournalPhase::Preparing;
        journal::write_preparing(&layout.journal_path, &preparing, uid)?;

        tree::stage_and_publish(
            &layout.staging_dir,
            &layout.tree_dir,
            &seed.files,
            &request.manifest_sha256,
            uid,
        )?;

        let mut prepared = desired.clone();
        prepared.phase = JournalPhase::Prepared;
        journal::write_prepared(&layout.journal_path, &prepared, uid)?;

        Ok(proof(request))
    }

    pub(crate) fn root_configured(&self) -> bool {
        self.root.is_some()
    }

    pub(crate) fn effective_uid(&self) -> anyhow::Result<u32> {
        self.uid_source.effective_uid()
    }

    /// Lock and validate one already-materialized task workspace.
    pub(crate) async fn lock_workspace(
        &self,
        environment_id: &str,
        task_id: uuid::Uuid,
        uid: u32,
    ) -> anyhow::Result<LockedNativeTaskWorkspace> {
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(UNSUPPORTED))?;
        paths::validate_root(root, uid)?;
        let layout = paths::layout(root, environment_id, task_id)?;
        let guard = self.lock_for(&layout.task_dir).lock_owned().await;
        paths::validate_private_dir(&layout.environment_dir, uid)?;
        paths::validate_private_dir(&layout.task_dir, uid)?;
        Ok(LockedNativeTaskWorkspace {
            layout,
            task_dir_present: true,
            _guard: guard,
        })
    }

    /// Lock a task path for stop without creating missing workspace directories.
    pub(crate) async fn lock_workspace_for_stop(
        &self,
        environment_id: &str,
        task_id: uuid::Uuid,
        uid: u32,
    ) -> anyhow::Result<LockedNativeTaskWorkspace> {
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(UNSUPPORTED))?;
        paths::validate_root(root, uid)?;
        let layout = paths::layout(root, environment_id, task_id)?;
        let guard = self.lock_for(&layout.task_dir).lock_owned().await;

        match std::fs::symlink_metadata(&layout.environment_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LockedNativeTaskWorkspace {
                    layout,
                    task_dir_present: false,
                    _guard: guard,
                });
            }
            Err(error) => {
                bail!("failed to inspect the native task environment directory: {error}");
            }
            Ok(_) => paths::validate_private_dir(&layout.environment_dir, uid)?,
        }

        let task_dir_present = match std::fs::symlink_metadata(&layout.task_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => bail!("failed to inspect the native task directory: {error}"),
            Ok(_) => {
                paths::validate_private_dir(&layout.task_dir, uid)?;
                true
            }
        };
        Ok(LockedNativeTaskWorkspace {
            layout,
            task_dir_present,
            _guard: guard,
        })
    }

    /// Reserve the per-task serialization mutex.
    fn lock_for(&self, task_dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let key = task_dir.to_string_lossy().to_string();
        let mut locks = self.locks.lock().expect("native task lock map");
        Arc::clone(
            locks
                .entry(key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }
}

impl LockedNativeTaskWorkspace {
    /// Validate identity and only the top-level manager-owned roots for stop.
    /// Descendants may contain task-written files and are never traversed.
    pub(crate) fn stop_journal(
        &self,
        expected: &Journal,
        uid: u32,
    ) -> anyhow::Result<Option<Journal>> {
        if !self.task_dir_present {
            return Ok(None);
        }
        let existing = journal::read(&self.layout.journal_path, uid)?
            .ok_or_else(|| anyhow::anyhow!("native task workspace exists without its journal"))?;
        if !existing.same_runtime_identity(expected)
            || existing.file_count as usize > runner_protocol::MAX_SEED_ENTRIES
            || existing.total_bytes as usize > runner_protocol::MAX_SEED_TOTAL_BYTES
        {
            bail!("native task stop journal identity does not match the stop job");
        }
        validate_owned_directory_if_present(&self.layout.tree_dir, uid)?;
        validate_owned_directory_if_present(&self.layout.staging_dir, uid)?;
        Ok(Some(existing))
    }

    pub(crate) fn verified_journal(&self, expected: &Journal, uid: u32) -> anyhow::Result<Journal> {
        let existing = journal::read(&self.layout.journal_path, uid)?
            .ok_or_else(|| anyhow::anyhow!("native task preparation journal is missing"))?;
        if !existing.same_runtime_identity(expected)
            || existing.file_count as usize > runner_protocol::MAX_SEED_ENTRIES
            || existing.total_bytes as usize > runner_protocol::MAX_SEED_TOTAL_BYTES
        {
            bail!("native task preparation journal identity does not match the start job");
        }
        Ok(existing)
    }

    pub(crate) fn verify_prepared_tree(&self, journal: &Journal, uid: u32) -> anyhow::Result<()> {
        let snapshot = tree::snapshot_published_tree(&self.layout.tree_dir, uid)?;
        if snapshot.manifest_sha256 != journal.manifest_sha256
            || snapshot.file_count != journal.file_count
            || snapshot.total_bytes != journal.total_bytes
        {
            bail!("prepared native task tree no longer matches its sealed manifest");
        }
        Ok(())
    }

    pub(crate) fn validate_running_tree(&self, uid: u32) -> anyhow::Result<()> {
        paths::validate_owned_tree_root(&self.layout.tree_dir, uid)
    }

    pub(crate) fn mark_starting(&self, journal: &Journal, uid: u32) -> anyhow::Result<()> {
        journal::write_starting(&self.layout.journal_path, journal, uid)
    }

    pub(crate) fn mark_running(&self, journal: &Journal, uid: u32) -> anyhow::Result<()> {
        journal::write_running(&self.layout.journal_path, journal, uid)
    }
}

fn validate_owned_directory_if_present(path: &Path, uid: u32) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => bail!("failed to inspect native task metadata directory: {error}"),
        Ok(_) => paths::validate_private_dir(path, uid),
    }
}

/// The typed proof the control plane matches against its sealed seed.
fn proof(request: &PrepareNativeTaskRequest) -> NativeTaskPreparedResult {
    NativeTaskPreparedResult {
        task_id: request.task_id,
        manifest_sha256: request.manifest_sha256.clone(),
        file_count: request.file_count,
        total_bytes: request.total_bytes,
        prepared: true,
    }
}

/// Require the fetched bundle to equal the sealed metadata.
fn verify_bundle(
    seed: &SeedBundle,
    request: &PrepareNativeTaskRequest,
) -> anyhow::Result<SeedBundle> {
    let validated = runner_protocol::validate_seed_bundle(&seed.files)
        .map_err(|_| anyhow::anyhow!("the fetched native task seed violates shared rules"))?;
    if seed.manifest_sha256 != validated.manifest_sha256
        || seed.file_count != validated.file_count
        || seed.total_bytes != validated.total_bytes
        || validated.manifest_sha256 != request.manifest_sha256
        || validated.file_count != request.file_count
        || validated.total_bytes != request.total_bytes
    {
        bail!("the fetched native task seed does not match the announced identity");
    }
    Ok(validated)
}

/// Convenience for tests and callers that need the trusted layout.
#[allow(dead_code)]
pub(crate) fn task_layout(
    root: &Path,
    request: &PrepareNativeTaskRequest,
) -> anyhow::Result<TaskLayout> {
    let (environment_id, _) =
        runner_protocol::execution_environment::as_execution_environment(&request.owner)
            .ok_or_else(|| {
                anyhow::anyhow!("native task preparation requires an environment owner")
            })?;
    paths::layout(root, environment_id, request.task_id)
}
