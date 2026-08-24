use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    ApplyPatchSummary, DirectoryEntry, FileFingerprint, ListDirectoryPageOutput,
    ListDirectoryPageParams, PathKind, ReadFilePageOutput, ReadFilePageParams, StatPathOutput,
    StatPathParams, WalkWorkspacePageOutput, WalkWorkspacePageParams, WorkspaceSdkError,
    patch::apply_patch, resolve_workspace_path, workspace_relative_display,
};

pub struct WorkspaceFileTools {
    workspace_root: PathBuf,
}

impl WorkspaceFileTools {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, WorkspaceSdkError> {
        let workspace_root = workspace_root.as_ref().canonicalize().map_err(|error| {
            WorkspaceSdkError::io(
                format!(
                    "failed to resolve workspace root {}",
                    workspace_root.as_ref().display()
                ),
                error,
            )
        })?;
        Ok(Self { workspace_root })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn apply_patch_text(&self, patch: &str) -> Result<ApplyPatchSummary, WorkspaceSdkError> {
        validate_patch_input(patch)?;
        apply_patch(&self.workspace_root, patch)
    }

    pub fn read_file_page(
        &self,
        params: &ReadFilePageParams,
    ) -> Result<ReadFilePageOutput, WorkspaceSdkError> {
        if params.start_line == 0 {
            return Err(WorkspaceSdkError::invalid_input(
                "start_line must be greater than or equal to 1",
            ));
        }
        if params.max_lines == 0 || params.max_bytes == 0 {
            return Err(WorkspaceSdkError::invalid_input(
                "max_lines and max_bytes must be greater than 0",
            ));
        }

        let absolute = resolve_workspace_path(&self.workspace_root, &params.path)?;
        let file = fs::File::open(&absolute).map_err(|error| {
            WorkspaceSdkError::io(format!("failed to read {}", params.path), error)
        })?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut total_lines = 0;
        let mut selected = String::new();
        let mut selected_line_count = 0;
        let mut truncated = false;
        while reader.read_line(&mut line).map_err(|error| {
            WorkspaceSdkError::io(format!("failed to read {}", params.path), error)
        })? > 0
        {
            total_lines += 1;
            if total_lines >= params.start_line
                && total_lines < params.start_line.saturating_add(params.max_lines)
            {
                if selected.len() < params.max_bytes {
                    let remaining = params.max_bytes - selected.len();
                    let take = utf8_prefix_len(line.as_bytes(), remaining);
                    selected.push_str(&line[..take]);
                    selected_line_count += 1;
                    if take < line.len() {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            } else if total_lines >= params.start_line.saturating_add(params.max_lines) {
                truncated = true;
            }
            line.clear();
        }

        Ok(ReadFilePageOutput {
            path: workspace_relative_display(&self.workspace_root, &absolute),
            content: selected,
            start_line: params.start_line,
            end_line: if selected_line_count > 0 {
                params.start_line + selected_line_count - 1
            } else {
                params.start_line
            },
            total_lines,
            truncated,
        })
    }

    pub fn list_directory_page(
        &self,
        params: &ListDirectoryPageParams,
    ) -> Result<ListDirectoryPageOutput, WorkspaceSdkError> {
        if params.limit == 0 {
            return Err(WorkspaceSdkError::invalid_input(
                "limit must be greater than 0",
            ));
        }
        let absolute = resolve_workspace_path(&self.workspace_root, &params.path)?;
        let mut paths = Vec::new();
        for entry in fs::read_dir(&absolute).map_err(|error| {
            WorkspaceSdkError::io(format!("failed to list {}", params.path), error)
        })? {
            let entry = entry.map_err(|error| {
                WorkspaceSdkError::io(format!("failed to list {}", params.path), error)
            })?;
            paths.push(entry.path());
        }
        paths.sort_by_cached_key(|path| workspace_relative_display(&self.workspace_root, path));
        let total_entries = paths.len();
        let entries = paths
            .into_iter()
            .skip(params.offset)
            .take(params.limit)
            .map(|path| DirectoryEntry {
                path: workspace_relative_display(&self.workspace_root, &path),
                kind: if path.is_dir() {
                    PathKind::Dir
                } else {
                    PathKind::File
                },
            })
            .collect();
        Ok(ListDirectoryPageOutput {
            path: workspace_relative_display(&self.workspace_root, &absolute),
            entries,
            offset: params.offset,
            total_entries,
            truncated: params.offset.saturating_add(params.limit) < total_entries,
        })
    }

    pub fn walk_workspace_page(
        &self,
        params: &WalkWorkspacePageParams,
    ) -> Result<WalkWorkspacePageOutput, WorkspaceSdkError> {
        if params.max_entries == 0 {
            return Err(WorkspaceSdkError::invalid_input(
                "max_entries must be greater than 0",
            ));
        }
        let absolute = resolve_workspace_path(&self.workspace_root, &params.path)?;
        let mut entries = Vec::new();
        let truncated = self.collect_entries_recursive_bounded(
            &absolute,
            &mut entries,
            0,
            params.max_entries,
            params.max_depth,
        )?;
        Ok(WalkWorkspacePageOutput {
            path: workspace_relative_display(&self.workspace_root, &absolute),
            entries,
            truncated,
        })
    }

    pub fn stat_path(&self, params: &StatPathParams) -> Result<StatPathOutput, WorkspaceSdkError> {
        let absolute = resolve_workspace_path(&self.workspace_root, &params.path)?;
        let metadata = fs::symlink_metadata(&absolute).map_err(|error| {
            WorkspaceSdkError::io(format!("failed to stat {}", params.path), error)
        })?;
        Ok(StatPathOutput {
            path: workspace_relative_display(&self.workspace_root, &absolute),
            kind: if metadata.is_dir() {
                PathKind::Dir
            } else {
                PathKind::File
            },
            size: metadata.len(),
            readonly: metadata.permissions().readonly(),
        })
    }

    pub fn fingerprint_path(
        &self,
        params: &StatPathParams,
    ) -> Result<FileFingerprint, WorkspaceSdkError> {
        let absolute = resolve_workspace_path(&self.workspace_root, &params.path)?;
        let metadata = fs::symlink_metadata(&absolute).map_err(|error| {
            WorkspaceSdkError::io(format!("failed to stat {}", params.path), error)
        })?;
        let (kind, sha256) = if metadata.is_file() {
            let mut file = fs::File::open(&absolute).map_err(|error| {
                WorkspaceSdkError::io(format!("failed to read {}", params.path), error)
            })?;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let read = std::io::Read::read(&mut file, &mut buffer).map_err(|error| {
                    WorkspaceSdkError::io(format!("failed to read {}", params.path), error)
                })?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            let digest = hasher.finalize();
            (
                PathKind::File,
                Some(digest.iter().map(|byte| format!("{byte:02x}")).collect()),
            )
        } else if metadata.is_dir() {
            (PathKind::Dir, None)
        } else {
            return Err(WorkspaceSdkError::invalid_input(
                "fingerprints are only supported for files and directories",
            ));
        };
        Ok(FileFingerprint {
            path: workspace_relative_display(&self.workspace_root, &absolute),
            kind,
            size: metadata.len(),
            sha256,
        })
    }

    fn collect_entries_recursive_bounded(
        &self,
        dir: &Path,
        output: &mut Vec<DirectoryEntry>,
        depth: usize,
        max_entries: usize,
        max_depth: Option<usize>,
    ) -> Result<bool, WorkspaceSdkError> {
        if output.len() >= max_entries || max_depth.is_some_and(|limit| depth > limit) {
            return Ok(true);
        }
        for entry in fs::read_dir(dir).map_err(|error| {
            WorkspaceSdkError::io(format!("failed to list {}", dir.display()), error)
        })? {
            let entry = entry.map_err(|error| {
                WorkspaceSdkError::io(format!("failed to list {}", dir.display()), error)
            })?;
            if output.len() >= max_entries {
                break;
            }
            let path = entry.path();
            let metadata = entry.file_type().map_err(|error| {
                WorkspaceSdkError::io(format!("failed to read type {}", path.display()), error)
            })?;
            let kind = if metadata.is_dir() {
                PathKind::Dir
            } else {
                PathKind::File
            };
            output.push(DirectoryEntry {
                path: workspace_relative_display(&self.workspace_root, &path),
                kind: kind.clone(),
            });
            if matches!(kind, PathKind::Dir)
                && self.collect_entries_recursive_bounded(
                    &path,
                    output,
                    depth + 1,
                    max_entries,
                    max_depth,
                )?
            {
                return Ok(true);
            }
        }
        Ok(output.len() >= max_entries)
    }
}

const MAX_PATCH_INPUT_BYTES: usize = 16 * 1024 * 1024;

fn validate_patch_input(patch: &str) -> Result<(), WorkspaceSdkError> {
    if patch.len() > MAX_PATCH_INPUT_BYTES {
        return Err(WorkspaceSdkError::invalid_input(
            "patch input exceeds maximum size",
        ));
    }
    Ok(())
}

fn utf8_prefix_len(bytes: &[u8], max_bytes: usize) -> usize {
    let mut end = bytes.len().min(max_bytes);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    end
}

#[cfg(test)]
#[path = "fs_tools_tests.rs"]
mod tests;
