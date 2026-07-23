use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ApplyPatchOperation, ParsedPatch, PatchOperation, WorkspaceSdkError, resolve_workspace_path,
};

use super::matching::{TextDocument, apply_hunks, count_lines};

#[derive(Debug)]
pub(crate) enum ExpectedState {
    Absent,
    Present(Vec<u8>),
}

#[derive(Debug)]
pub(crate) enum PlannedAction {
    Write {
        path: PathBuf,
        relative_path: String,
        expected: ExpectedState,
        content: Vec<u8>,
        permissions: Option<fs::Permissions>,
    },
    Delete {
        path: PathBuf,
        relative_path: String,
        expected: Vec<u8>,
    },
    Move {
        source: PathBuf,
        source_relative_path: String,
        source_expected: Vec<u8>,
        target: PathBuf,
        target_relative_path: String,
        content: Vec<u8>,
        permissions: Option<fs::Permissions>,
    },
}

#[derive(Debug)]
pub(crate) struct PlannedChange {
    pub operation: ApplyPatchOperation,
    pub path: String,
    pub destination: Option<String>,
    pub action: PlannedAction,
    pub added_lines: usize,
    pub deleted_lines: usize,
}

pub(crate) fn plan_patch(
    root: &Path,
    parsed: &ParsedPatch,
) -> Result<Vec<PlannedChange>, WorkspaceSdkError> {
    let mut reserved = HashSet::new();
    let mut planned = Vec::with_capacity(parsed.operations.len());

    for operation in &parsed.operations {
        match operation {
            PatchOperation::Add { path, lines } => {
                let absolute = resolve_workspace_path(root, path)?;
                reject_reserved(&mut reserved, &absolute, path)?;
                if path_exists(&absolute).map_err(|error| {
                    WorkspaceSdkError::io(format!("failed to inspect {path}"), error)
                })? {
                    return Err(WorkspaceSdkError::invalid_input(format!(
                        "cannot add existing file: {path}"
                    )));
                }
                let content = render_added_lines(lines).into_bytes();
                planned.push(PlannedChange {
                    operation: ApplyPatchOperation::Add,
                    path: path.clone(),
                    destination: None,
                    action: PlannedAction::Write {
                        path: absolute,
                        relative_path: path.clone(),
                        expected: ExpectedState::Absent,
                        content,
                        permissions: None,
                    },
                    added_lines: lines.len(),
                    deleted_lines: 0,
                });
            }
            PatchOperation::Delete { path } => {
                let absolute = resolve_workspace_path(root, path)?;
                reject_reserved(&mut reserved, &absolute, path)?;
                let snapshot = read_snapshot(root, &absolute, path)?;
                planned.push(PlannedChange {
                    operation: ApplyPatchOperation::Delete,
                    path: path.clone(),
                    destination: None,
                    action: PlannedAction::Delete {
                        path: absolute,
                        relative_path: path.clone(),
                        expected: snapshot.bytes.clone(),
                    },
                    added_lines: 0,
                    deleted_lines: count_lines(&snapshot.bytes),
                });
            }
            PatchOperation::Update {
                path,
                move_to,
                hunks,
            } => {
                let source = resolve_workspace_path(root, path)?;
                reject_reserved(&mut reserved, &source, path)?;
                let snapshot = read_snapshot(root, &source, path)?;
                let document = TextDocument::decode(&snapshot.bytes).map_err(|message| {
                    WorkspaceSdkError::invalid_input(format!("failed to update {path}: {message}"))
                })?;
                let (updated, added_lines, deleted_lines) =
                    apply_hunks(&document, hunks).map_err(|message| {
                        WorkspaceSdkError::invalid_input(format!(
                            "failed to apply hunks to {path}: {message}"
                        ))
                    })?;
                let content = updated.into_bytes();

                if let Some(target_relative_path) = move_to {
                    let target = resolve_workspace_path(root, target_relative_path)?;
                    if target == source {
                        return Err(WorkspaceSdkError::invalid_input(format!(
                            "move source and destination must differ: {path}"
                        )));
                    }
                    reject_reserved(&mut reserved, &target, target_relative_path)?;
                    if path_exists(&target).map_err(|error| {
                        WorkspaceSdkError::io(
                            format!("failed to inspect {target_relative_path}"),
                            error,
                        )
                    })? {
                        return Err(WorkspaceSdkError::invalid_input(format!(
                            "cannot move over existing file: {target_relative_path}"
                        )));
                    }
                    planned.push(PlannedChange {
                        operation: ApplyPatchOperation::Move,
                        path: path.clone(),
                        destination: Some(target_relative_path.clone()),
                        action: PlannedAction::Move {
                            source,
                            source_relative_path: path.clone(),
                            source_expected: snapshot.bytes,
                            target,
                            target_relative_path: target_relative_path.clone(),
                            content,
                            permissions: Some(snapshot.permissions),
                        },
                        added_lines,
                        deleted_lines,
                    });
                } else {
                    planned.push(PlannedChange {
                        operation: ApplyPatchOperation::Update,
                        path: path.clone(),
                        destination: None,
                        action: PlannedAction::Write {
                            path: source,
                            relative_path: path.clone(),
                            expected: ExpectedState::Present(snapshot.bytes),
                            content,
                            permissions: Some(snapshot.permissions),
                        },
                        added_lines,
                        deleted_lines,
                    });
                }
            }
        }
    }
    Ok(planned)
}

#[derive(Debug)]
struct FileSnapshot {
    bytes: Vec<u8>,
    permissions: fs::Permissions,
}

fn read_snapshot(
    _root: &Path,
    path: &Path,
    display_path: &str,
) -> Result<FileSnapshot, WorkspaceSdkError> {
    let metadata = fs::metadata(path).map_err(|error| {
        WorkspaceSdkError::io(format!("failed to inspect {display_path}"), error)
    })?;
    if !metadata.is_file() {
        return Err(WorkspaceSdkError::invalid_input(format!(
            "patch target is not a regular file: {display_path}"
        )));
    }
    let bytes = fs::read(path)
        .map_err(|error| WorkspaceSdkError::io(format!("failed to read {display_path}"), error))?;
    Ok(FileSnapshot {
        bytes,
        permissions: metadata.permissions(),
    })
}

pub(crate) fn path_exists(path: &Path) -> Result<bool, std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn reject_reserved(
    reserved: &mut HashSet<PathBuf>,
    path: &Path,
    display_path: &str,
) -> Result<(), WorkspaceSdkError> {
    if !reserved.insert(path.to_path_buf()) {
        return Err(WorkspaceSdkError::invalid_input(format!(
            "patch contains conflicting operations for: {display_path}"
        )));
    }
    Ok(())
}

fn render_added_lines(lines: &[String]) -> String {
    let mut content = lines.join("\n");
    content.push('\n');
    content
}

#[cfg(test)]
mod tests {
    use super::render_added_lines;

    #[test]
    fn renders_added_files_with_a_trailing_newline() {
        assert_eq!(render_added_lines(&["one".to_string()]), "one\n");
    }
}
