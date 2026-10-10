//! The native-task materializer (issue 753 P2b).
//!
//! One store per manager. It owns the optional trusted root, the effective-uid
//! source, and a per-task serial mutex so two durable preparations for the same
//! task can never both publish. It is filesystem only: no container, no port,
//! no agent, no canonical project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
