use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    PatchCode, PatchDiagnostic, PatchEffect, PatchStage,
    patch::plan::{ExpectedState, PlannedAction, PlannedChange, path_exists},
    resolve_workspace_path,
};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn real_rename(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

pub(crate) fn real_remove(path: &Path) -> std::io::Result<()> {
    fs::remove_file(path)
}

/// Filesystem operations used by the commit path. Production always passes
/// [`FsOps::REAL`]; only in-crate tests may substitute a failing operation to
/// exercise an otherwise non-deterministic cleanup/rename branch.
#[derive(Clone, Copy)]
pub(crate) struct FsOps {
    rename: fn(&Path, &Path) -> std::io::Result<()>,
    remove: fn(&Path) -> std::io::Result<()>,
}

impl FsOps {
    pub(crate) const REAL: Self = Self {
        rename: real_rename,
        remove: real_remove,
    };

    #[cfg(test)]
    pub(crate) const fn new(
        rename: fn(&Path, &Path) -> std::io::Result<()>,
        remove: fn(&Path) -> std::io::Result<()>,
    ) -> Self {
        Self { rename, remove }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommitOutcome {
    pub effect: PatchEffect,
    pub error: Option<PatchCode>,
}

impl CommitOutcome {
    fn changed() -> Self {
        Self {
            effect: PatchEffect::Changed,
            error: None,
        }
    }

    fn failure(effect: PatchEffect, code: PatchCode) -> Self {
        Self {
            effect,
            error: Some(code),
        }
    }
}

/// What `ensure_parent` observed about the target's parent directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParentEffect {
    Existing,
    Created,
    Unknown,
}

impl ParentEffect {
    /// A later failure only upgrades an otherwise unchanged outcome when this
    /// commit created the parent directory. Unknown always stays unknown.
    fn after_failure(self, effect: PatchEffect) -> PatchEffect {
        match (self, effect) {
            (_, PatchEffect::Unknown) => PatchEffect::Unknown,
            (Self::Created, PatchEffect::Unchanged) => PatchEffect::Changed,
            _ => effect,
        }
    }
}

pub(crate) fn commit_change(root: &Path, change: &PlannedChange, ops: FsOps) -> CommitOutcome {
    match &change.action {
        PlannedAction::Write {
            path,
            relative_path,
            expected,
            content,
            permissions,
        } => {
            if let Err(code) = verify_path(root, relative_path, path, PatchCode::PatchTargetChanged)
            {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            if let Err(code) = verify_expected(path, expected, PatchCode::PatchTargetChanged) {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            let parent = match ensure_parent(path) {
                Ok(parent) => parent,
                Err(code) => return CommitOutcome::failure(PatchEffect::Unknown, code),
            };
            if let Err(code) = verify_path(root, relative_path, path, PatchCode::PatchTargetChanged)
            {
                return CommitOutcome::failure(parent.after_failure(PatchEffect::Unchanged), code);
            }
            let effect = match expected {
                ExpectedState::Absent => PatchEffect::Changed,
                ExpectedState::Present(bytes) if bytes == content => PatchEffect::Unchanged,
                ExpectedState::Present(_) => PatchEffect::Changed,
            };
            match atomic_write(path, content, permissions.as_ref(), ops) {
                Ok(()) => CommitOutcome {
                    effect,
                    error: None,
                },
                Err(failure) => failure.outcome(parent),
            }
        }
        PlannedAction::Delete {
            path,
            relative_path,
            expected,
        } => {
            if let Err(code) = verify_path(root, relative_path, path, PatchCode::PatchTargetChanged)
            {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            if let Err(code) = verify_expected(
                path,
                &ExpectedState::Present(expected.clone()),
                PatchCode::PatchTargetChanged,
            ) {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            match (ops.remove)(path) {
                Ok(()) => CommitOutcome::changed(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    CommitOutcome::failure(PatchEffect::Unknown, PatchCode::PatchWriteFailed)
                }
                Err(_) => {
                    CommitOutcome::failure(PatchEffect::Unchanged, PatchCode::PatchWriteFailed)
                }
            }
        }
        PlannedAction::Move {
            source,
            source_relative_path,
            source_expected,
            target,
            target_relative_path,
            content,
            permissions,
        } => {
            if let Err(code) = verify_path(
                root,
                source_relative_path,
                source,
                PatchCode::PatchSourceChanged,
            ) {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            if let Err(code) = verify_expected(
                source,
                &ExpectedState::Present(source_expected.clone()),
                PatchCode::PatchSourceChanged,
            ) {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            if let Err(code) = verify_path(
                root,
                target_relative_path,
                target,
                PatchCode::PatchTargetChanged,
            ) {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            if let Err(code) = verify_expected(
                target,
                &ExpectedState::Absent,
                PatchCode::PatchTargetChanged,
            ) {
                return CommitOutcome::failure(PatchEffect::Unchanged, code);
            }
            let parent = match ensure_parent(target) {
                Ok(parent) => parent,
                Err(code) => return CommitOutcome::failure(PatchEffect::Unknown, code),
            };
            if let Err(code) = verify_path(
                root,
                target_relative_path,
                target,
                PatchCode::PatchTargetChanged,
            ) {
                return CommitOutcome::failure(parent.after_failure(PatchEffect::Unchanged), code);
            }
            match atomic_write(target, content, permissions.as_ref(), ops) {
                Ok(()) => {}
                Err(failure) => return failure.outcome(parent),
            }
            if let Err(code) = verify_expected(
                source,
                &ExpectedState::Present(source_expected.clone()),
                PatchCode::PatchSourceChanged,
            ) {
                return CommitOutcome::failure(PatchEffect::Changed, code);
            }
            match (ops.remove)(source) {
                Ok(()) => CommitOutcome::changed(),
                Err(_) => CommitOutcome::failure(PatchEffect::Changed, PatchCode::PatchWriteFailed),
            }
        }
    }
}

struct WriteFailure {
    effect: PatchEffect,
}

impl WriteFailure {
    fn outcome(self, parent: ParentEffect) -> CommitOutcome {
        CommitOutcome::failure(
            parent.after_failure(self.effect),
            PatchCode::PatchWriteFailed,
        )
    }
}

fn verify_path(
    root: &Path,
    relative_path: &str,
    expected: &Path,
    mismatch: PatchCode,
) -> Result<(), PatchCode> {
    let current = resolve_workspace_path(root, relative_path).map_err(|_| mismatch)?;
    if current != expected {
        return Err(mismatch);
    }
    Ok(())
}

fn verify_expected(
    path: &Path,
    expected: &ExpectedState,
    mismatch: PatchCode,
) -> Result<(), PatchCode> {
    match expected {
        ExpectedState::Absent => {
            let exists = path_exists(path).map_err(|_| mismatch)?;
            if exists {
                return Err(mismatch);
            }
        }
        ExpectedState::Present(bytes) => {
            let current = fs::read(path).map_err(|_| mismatch)?;
            if current != *bytes {
                return Err(mismatch);
            }
        }
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<ParentEffect, PatchCode> {
    let Some(parent) = path.parent() else {
        return Ok(ParentEffect::Existing);
    };
    let observed = match fs::metadata(parent) {
        Ok(_) => ParentEffect::Existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ParentEffect::Created,
        Err(_) => ParentEffect::Unknown,
    };
    fs::create_dir_all(parent).map_err(|_| PatchCode::PatchWriteFailed)?;
    Ok(observed)
}

fn atomic_write(
    path: &Path,
    content: &[u8],
    permissions: Option<&fs::Permissions>,
    ops: FsOps,
) -> Result<(), WriteFailure> {
    let parent = path.parent().ok_or(WriteFailure {
        effect: PatchEffect::Unchanged,
    })?;
    let temp_path = temporary_path(parent, path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|_| WriteFailure {
            effect: PatchEffect::Unchanged,
        })?;

    let result = (|| {
        file.write_all(content).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        drop(file);
        if let Some(permissions) = permissions {
            fs::set_permissions(&temp_path, permissions.clone()).map_err(|_| ())?;
        }
        (ops.rename)(&temp_path, path).map_err(|_| ())
    })();
    if result.is_err() {
        return Err(WriteFailure {
            effect: cleanup_effect(ops, &temp_path),
        });
    }
    Ok(())
}

fn cleanup_effect(ops: FsOps, temp_path: &Path) -> PatchEffect {
    match (ops.remove)(temp_path) {
        Ok(()) => PatchEffect::Unchanged,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PatchEffect::Unchanged,
        Err(_) => PatchEffect::Unknown,
    }
}

fn temporary_path(parent: &Path, target: &Path) -> PathBuf {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("patch-target");
    parent.join(format!(
        ".{name}.desk-foreman-patch-{}-{counter}",
        std::process::id()
    ))
}

pub(crate) fn commit_diagnostic(change: &PlannedChange, code: PatchCode) -> PatchDiagnostic {
    PatchDiagnostic::new(code, PatchStage::Commit)
        .with_path(Some(change.path.clone()))
        .with_operation(Some(change.operation))
}
