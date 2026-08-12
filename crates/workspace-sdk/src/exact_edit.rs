use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{WorkspaceSdkError, resolve_workspace_path, workspace_relative_display};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ExactEditRequest {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub replace_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExactEditResult {
    pub path: String,
    pub replacements: usize,
    pub added_lines: usize,
    pub deleted_lines: usize,
    pub sha256: String,
}

#[derive(Debug, Error)]
pub enum ExactEditError {
    #[error("old_text must not be empty; create new files with apply_patch")]
    OldTextEmpty,
    #[error("old_text equals new_text; nothing to replace")]
    NoOp,
    #[error("{path} is not valid UTF-8")]
    NotUtf8 { path: String },
    #[error("old_text was not found in {path}; re-read the file and retry")]
    ContextNotFound { path: String },
    #[error(
        "old_text matches {matches} times in {path}; provide more context or set replace_all=true"
    )]
    Ambiguous { path: String, matches: usize },
    #[error("{0}")]
    Workspace(#[from] WorkspaceSdkError),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl ExactEditError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::OldTextEmpty => "edit_old_text_empty",
            Self::NoOp => "edit_noop",
            Self::NotUtf8 { .. } => "edit_not_utf8",
            Self::ContextNotFound { .. } => "edit_context_not_found",
            Self::Ambiguous { .. } => "edit_context_ambiguous",
            Self::Workspace(_) => "workspace_path_invalid",
            Self::Io { .. } => "edit_io_failure",
        }
    }
}

pub(crate) fn edit_file(
    root: &Path,
    request: &ExactEditRequest,
) -> Result<ExactEditResult, ExactEditError> {
    if request.old_text.is_empty() {
        return Err(ExactEditError::OldTextEmpty);
    }
    if request.old_text == request.new_text {
        return Err(ExactEditError::NoOp);
    }

    let absolute = resolve_workspace_path(root, &request.path)?;
    let bytes = fs::read(&absolute).map_err(|source| ExactEditError::Io {
        context: format!("failed to read {}", request.path),
        source,
    })?;
    let had_bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let content_bytes = if had_bom { &bytes[3..] } else { &bytes };
    let content =
        String::from_utf8(content_bytes.to_vec()).map_err(|_| ExactEditError::NotUtf8 {
            path: request.path.clone(),
        })?;
    let line_ending = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let old_text = normalize_line_endings(&request.old_text, line_ending);
    let new_text = normalize_line_endings(&request.new_text, line_ending);
    let matches = match_indices(&content, &old_text);
    if matches.is_empty() {
        return Err(ExactEditError::ContextNotFound {
            path: request.path.clone(),
        });
    }
    if matches.len() > 1 && !request.replace_all {
        return Err(ExactEditError::Ambiguous {
            path: request.path.clone(),
            matches: matches.len(),
        });
    }

    let (replaced, replacements) = replace_matches(&content, &old_text, &new_text, &matches);
    let mut output = Vec::with_capacity(replaced.len() + usize::from(had_bom) * 3);
    if had_bom {
        output.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    output.extend_from_slice(replaced.as_bytes());
    let permissions = fs::metadata(&absolute)
        .map_err(|source| ExactEditError::Io {
            context: format!("failed to inspect {}", request.path),
            source,
        })?
        .permissions();
    atomic_write(&absolute, &output, &permissions)?;

    Ok(ExactEditResult {
        path: workspace_relative_display(root, &absolute),
        replacements,
        added_lines: line_count(&new_text),
        deleted_lines: line_count(&old_text),
        sha256: sha256(&output),
    })
}

fn normalize_line_endings(text: &str, ending: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', ending)
}

fn match_indices(content: &str, needle: &str) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut offset = 0;
    while let Some(relative) = content[offset..].find(needle) {
        let index = offset + relative;
        indices.push(index);
        offset = index + needle.len();
    }
    indices
}

fn replace_matches(
    content: &str,
    old_text: &str,
    new_text: &str,
    indices: &[usize],
) -> (String, usize) {
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    for index in indices {
        output.push_str(&content[cursor..*index]);
        output.push_str(new_text);
        cursor = *index + old_text.len();
    }
    output.push_str(&content[cursor..]);
    (output, indices.len())
}

fn atomic_write(
    path: &Path,
    content: &[u8],
    permissions: &fs::Permissions,
) -> Result<(), ExactEditError> {
    let parent = path.parent().ok_or_else(|| ExactEditError::Io {
        context: format!("file has no parent directory: {}", path.display()),
        source: std::io::Error::other("missing parent directory"),
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{name}.desk-foreman-edit-{}-{counter}",
        std::process::id()
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|source| ExactEditError::Io {
                context: format!("failed to create {}", temp.display()),
                source,
            })?;
        file.write_all(content)
            .map_err(|source| ExactEditError::Io {
                context: format!("failed to write {}", temp.display()),
                source,
            })?;
        file.sync_all().map_err(|source| ExactEditError::Io {
            context: format!("failed to flush {}", temp.display()),
            source,
        })?;
        drop(file);
        fs::set_permissions(&temp, permissions.clone()).map_err(|source| ExactEditError::Io {
            context: format!("failed to preserve permissions for {}", path.display()),
            source,
        })?;
        fs::rename(&temp, path).map_err(|source| ExactEditError::Io {
            context: format!("failed to replace {}", path.display()),
            source,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

fn sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn request(old_text: &str, new_text: &str, replace_all: bool) -> ExactEditRequest {
        ExactEditRequest {
            path: "src/lib.rs".to_string(),
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
            replace_all,
        }
    }

    #[test]
    fn edits_atomically_and_preserves_bom_and_crlf() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("src")).expect("src");
        fs::write(
            temp.path().join("src/lib.rs"),
            b"\xef\xbb\xbffn main() {\r\n  old\r\n}\r\n",
        )
        .expect("file");

        let result = edit_file(temp.path(), &request("old\n", "new\n", false)).expect("edit");
        assert_eq!(result.path, "src/lib.rs");
        assert_eq!(result.replacements, 1);
        assert_eq!(
            fs::read(temp.path().join("src/lib.rs")).expect("read"),
            b"\xef\xbb\xbffn main() {\r\n  new\r\n}\r\n"
        );
    }

    #[test]
    fn rejects_ambiguous_context_without_mutating_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("src.rs"), "same\nsame\n").expect("file");
        let mut request = request("same", "changed", false);
        request.path = "src.rs".to_string();
        let error = edit_file(temp.path(), &request).expect_err("ambiguous");
        assert!(matches!(
            error,
            ExactEditError::Ambiguous { matches: 2, .. }
        ));
        assert_eq!(
            fs::read_to_string(temp.path().join("src.rs")).unwrap(),
            "same\nsame\n"
        );
    }
}
