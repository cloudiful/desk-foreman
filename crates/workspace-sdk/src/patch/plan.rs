use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ApplyPatchOperation, ParsedPatch, PatchCode, PatchDiagnostic, PatchOperation, PatchStage,
    patch_path_is_protected, resolve_workspace_path,
};

use super::matching::{TextDocument, apply_hunks};

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
}

pub(crate) fn plan_patch(
    root: &Path,
    parsed: &ParsedPatch,
) -> Result<Vec<PlannedChange>, PatchDiagnostic> {
    let mut reserved = HashSet::new();
    let mut planned = Vec::with_capacity(parsed.operations.len());

    for operation in &parsed.operations {
        match operation {
            PatchOperation::Add { path, lines } => {
                let absolute = resolve_patch_path(root, path, ApplyPatchOperation::Add)?;
                reject_protected(path, ApplyPatchOperation::Add)?;
                reject_reserved(&mut reserved, &absolute, path, ApplyPatchOperation::Add)?;
                if path_exists(&absolute)
                    .map_err(|_| path_failure(path, ApplyPatchOperation::Add))?
                {
                    return Err(target_failure(
                        PatchCode::PatchTargetExists,
                        path,
                        ApplyPatchOperation::Add,
                    ));
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
                });
            }
            PatchOperation::Delete { path } => {
                let absolute = resolve_patch_path(root, path, ApplyPatchOperation::Delete)?;
                reject_protected(path, ApplyPatchOperation::Delete)?;
                reject_reserved(&mut reserved, &absolute, path, ApplyPatchOperation::Delete)?;
                let snapshot = read_snapshot(&absolute, path, ApplyPatchOperation::Delete)?;
                planned.push(PlannedChange {
                    operation: ApplyPatchOperation::Delete,
                    path: path.clone(),
                    destination: None,
                    action: PlannedAction::Delete {
                        path: absolute,
                        relative_path: path.clone(),
                        expected: snapshot.bytes,
                    },
                });
            }
            PatchOperation::Update {
                path,
                move_to,
                hunks,
            } => {
                let update_operation = if move_to.is_some() {
                    ApplyPatchOperation::Move
                } else {
                    ApplyPatchOperation::Update
                };
                let source = resolve_patch_path(root, path, update_operation)?;
                reject_protected(path, update_operation)?;
                reject_reserved(&mut reserved, &source, path, update_operation)?;
                let snapshot = read_snapshot(&source, path, update_operation)?;
                let document = TextDocument::decode(&snapshot.bytes).map_err(|diagnostic| {
                    diagnostic
                        .with_path(Some(path.clone()))
                        .with_operation(Some(update_operation))
                })?;
                let updated = apply_hunks(&document, hunks).map_err(|diagnostic| {
                    diagnostic
                        .with_path(Some(path.clone()))
                        .with_operation(Some(update_operation))
                })?;
                let content = updated.into_bytes();

                if let Some(target_relative_path) = move_to {
                    let target =
                        resolve_patch_path(root, target_relative_path, ApplyPatchOperation::Move)?;
                    reject_protected(target_relative_path, ApplyPatchOperation::Move)?;
                    if target == source {
                        return Err(move_failure(
                            target_relative_path,
                            ApplyPatchOperation::Move,
                        ));
                    }
                    reject_reserved(
                        &mut reserved,
                        &target,
                        target_relative_path,
                        ApplyPatchOperation::Move,
                    )?;
                    if path_exists(&target).map_err(|_| {
                        path_failure(target_relative_path, ApplyPatchOperation::Move)
                    })? {
                        return Err(move_failure(
                            target_relative_path,
                            ApplyPatchOperation::Move,
                        ));
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

fn resolve_patch_path(
    root: &Path,
    relative: &str,
    operation: ApplyPatchOperation,
) -> Result<PathBuf, PatchDiagnostic> {
    resolve_workspace_path(root, relative).map_err(|_| {
        PatchDiagnostic::new(PatchCode::PatchPathInvalid, PatchStage::Plan)
            .with_operation(Some(operation))
    })
}

fn reject_protected(relative: &str, operation: ApplyPatchOperation) -> Result<(), PatchDiagnostic> {
    if patch_path_is_protected(relative) {
        return Err(
            PatchDiagnostic::new(PatchCode::PatchPathProtected, PatchStage::Plan)
                .with_path(Some(relative.to_string()))
                .with_operation(Some(operation)),
        );
    }
    Ok(())
}

fn read_snapshot(
    path: &Path,
    display_path: &str,
    operation: ApplyPatchOperation,
) -> Result<FileSnapshot, PatchDiagnostic> {
    let metadata = fs::metadata(path)
        .map_err(|_| target_failure(PatchCode::PatchTargetMissing, display_path, operation))?;
    if !metadata.is_file() {
        return Err(target_failure(
            PatchCode::PatchTargetNotRegular,
            display_path,
            operation,
        ));
    }
    let bytes = fs::read(path)
        .map_err(|_| target_failure(PatchCode::PatchTargetMissing, display_path, operation))?;
    Ok(FileSnapshot {
        bytes,
        permissions: metadata.permissions(),
    })
}

pub(crate) fn path_exists(path: &Path) -> Result<bool, std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.kind() == std::io::ErrorKind::NotADirectory =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn reject_reserved(
    reserved: &mut HashSet<PathBuf>,
    path: &Path,
    display_path: &str,
    operation: ApplyPatchOperation,
) -> Result<(), PatchDiagnostic> {
    if !reserved.insert(path.to_path_buf()) {
        return Err(target_failure(
            PatchCode::PatchDuplicateTarget,
            display_path,
            operation,
        ));
    }
    Ok(())
}

fn target_failure(code: PatchCode, path: &str, operation: ApplyPatchOperation) -> PatchDiagnostic {
    PatchDiagnostic::new(code, PatchStage::Plan)
        .with_path(Some(path.to_string()))
        .with_operation(Some(operation))
}

fn path_failure(path: &str, operation: ApplyPatchOperation) -> PatchDiagnostic {
    PatchDiagnostic::new(PatchCode::PatchPathInvalid, PatchStage::Plan)
        .with_path(Some(path.to_string()))
        .with_operation(Some(operation))
}

fn move_failure(path: &str, operation: ApplyPatchOperation) -> PatchDiagnostic {
    PatchDiagnostic::new(PatchCode::PatchMoveConflict, PatchStage::Plan)
        .with_path(Some(path.to_string()))
        .with_operation(Some(operation))
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
