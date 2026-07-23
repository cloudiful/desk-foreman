use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    patch::plan::{ExpectedState, PlannedAction, PlannedChange, path_exists},
    resolve_workspace_path,
};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn commit_change(root: &Path, change: &PlannedChange) -> Result<(), String> {
    match &change.action {
        PlannedAction::Write {
            path,
            relative_path,
            expected,
            content,
            permissions,
        } => {
            verify_path(root, relative_path, path)?;
            verify_expected(path, expected)?;
            ensure_parent(path)?;
            verify_path(root, relative_path, path)?;
            atomic_write(path, content, permissions.as_ref())
        }
        PlannedAction::Delete {
            path,
            relative_path,
            expected,
        } => {
            verify_path(root, relative_path, path)?;
            verify_expected(path, &ExpectedState::Present(expected.clone()))?;
            fs::remove_file(path)
                .map_err(|error| format!("failed to remove {}: {error}", path.display()))
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
            verify_path(root, source_relative_path, source)?;
            verify_expected(source, &ExpectedState::Present(source_expected.clone()))?;
            verify_path(root, target_relative_path, target)?;
            verify_expected(target, &ExpectedState::Absent)?;
            ensure_parent(target)?;
            verify_path(root, target_relative_path, target)?;
            atomic_write(target, content, permissions.as_ref())?;
            verify_expected(source, &ExpectedState::Present(source_expected.clone()))?;
            fs::remove_file(source).map_err(|error| {
                format!(
                    "failed to remove moved source {}: {error}",
                    source.display()
                )
            })
        }
    }
}

fn verify_path(root: &Path, relative_path: &str, expected: &Path) -> Result<(), String> {
    let current = resolve_workspace_path(root, relative_path)
        .map_err(|error| format!("failed to revalidate {relative_path}: {error}"))?;
    if current != expected {
        return Err(format!(
            "workspace path changed while applying patch: {relative_path}"
        ));
    }
    Ok(())
}

fn verify_expected(path: &Path, expected: &ExpectedState) -> Result<(), String> {
    match expected {
        ExpectedState::Absent => {
            if path_exists(path)
                .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?
            {
                return Err(format!(
                    "target appeared while applying patch: {}",
                    path.display()
                ));
            }
        }
        ExpectedState::Present(bytes) => {
            let current = fs::read(path)
                .map_err(|error| format!("failed to re-read {}: {error}", path.display()))?;
            if current != *bytes {
                return Err(format!(
                    "file changed while applying patch: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))
}

fn atomic_write(
    path: &Path,
    content: &[u8],
    permissions: Option<&fs::Permissions>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("file has no parent directory: {}", path.display()))?;
    let temp_path = temporary_path(parent, path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|error| {
            format!(
                "failed to create temporary file {}: {error}",
                temp_path.display()
            )
        })?;

    let result = (|| {
        file.write_all(content).map_err(|error| {
            format!(
                "failed to write temporary file {}: {error}",
                temp_path.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "failed to flush temporary file {}: {error}",
                temp_path.display()
            )
        })?;
        drop(file);
        if let Some(permissions) = permissions {
            fs::set_permissions(&temp_path, permissions.clone()).map_err(|error| {
                format!(
                    "failed to preserve permissions for {}: {error}",
                    path.display()
                )
            })?;
        }
        fs::rename(&temp_path, path)
            .map_err(|error| format!("failed to atomically replace {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
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
