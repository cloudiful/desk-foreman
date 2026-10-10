//! Per-environment filesystem lock (issue 742 P1).
//!
//! One `create_new` reservation file per environment with an explicit holder
//! token. An existing or crash-held lock fails closed instead of being
//! stolen: there is no stale-PID cleanup and no `Drop`-based removal, so a
//! cancelled or crashed holder leaves the reservation for explicit operator
//! recovery. Release removes the file only when it still carries the
//! holder's own token.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static HOLDER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Exclusive reservation for one environment's state directory.
#[derive(Debug)]
pub(crate) struct EnvFileLock {
    path: PathBuf,
    holder: String,
}

impl EnvFileLock {
    /// Reserve the lock, failing closed when any reservation already exists.
    pub(crate) fn acquire(path: &Path, holder: String) -> anyhow::Result<Self> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => anyhow::anyhow!(
                    "execution environment state lock {} is already held; refusing to run (a crash-held lock requires explicit operator recovery)",
                    path.display()
                ),
                _ => anyhow::anyhow!(
                    "failed to acquire execution environment state lock {}: {error}",
                    path.display()
                ),
            })?;
        file.write_all(holder.as_bytes()).map_err(|error| {
            anyhow::anyhow!(
                "failed to write execution environment state lock {}: {error}",
                path.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            anyhow::anyhow!(
                "failed to sync execution environment state lock {}: {error}",
                path.display()
            )
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            holder,
        })
    }

    /// Remove the reservation only when this holder still owns it.
    pub(crate) fn release(&self) -> anyhow::Result<()> {
        let mut contents = String::new();
        File::open(&self.path)
            .and_then(|mut file| file.read_to_string(&mut contents))
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to read execution environment state lock {}: {error}",
                    self.path.display()
                )
            })?;
        if contents != self.holder {
            anyhow::bail!(
                "execution environment state lock {} is held by another identity; refusing to remove it",
                self.path.display()
            );
        }
        std::fs::remove_file(&self.path).map_err(|error| {
            anyhow::anyhow!(
                "failed to release execution environment state lock {}: {error}",
                self.path.display()
            )
        })?;
        if let Some(dir) = self.path.parent() {
            if let Ok(handle) = File::open(dir) {
                handle.sync_all().map_err(|error| {
                    anyhow::anyhow!(
                        "failed to sync execution environment state directory {}: {error}",
                        dir.display()
                    )
                })?;
            }
        }
        Ok(())
    }
}

/// Release an owned lock after an operation, preserving a successful result
/// only when release also succeeded. A failed release on an already-failed
/// operation is logged and the original error is kept.
pub(crate) fn release_and_finish<T>(
    lock: EnvFileLock,
    outcome: anyhow::Result<T>,
) -> anyhow::Result<T> {
    match lock.release() {
        Ok(()) => outcome,
        Err(error) => {
            if outcome.is_ok() {
                return Err(error);
            }
            tracing::error!(
                error = %error,
                "failed to release execution environment state lock after operation error"
            );
            outcome
        }
    }
}

/// Holder identity for one reservation: manager id plus this process, never
/// a secret. The nonce keeps repeated attempts distinguishable.
pub(crate) fn lock_holder_token(manager_id: &str) -> String {
    let counter = HOLDER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "manager={manager_id};pid={};nonce={nanos}-{counter}",
        std::process::id()
    )
}
