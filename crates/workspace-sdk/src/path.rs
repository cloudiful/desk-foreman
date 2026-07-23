use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::WorkspaceSdkError;

pub fn resolve_workspace_path(root: &Path, relative: &str) -> Result<PathBuf, WorkspaceSdkError> {
    let root = root.canonicalize().map_err(|error| {
        WorkspaceSdkError::io(
            format!("failed to resolve workspace root {}", root.display()),
            error,
        )
    })?;
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(WorkspaceSdkError::invalid_input(
            "absolute paths are not allowed",
        ));
    }

    for component in rel.components() {
        match component {
            Component::CurDir | Component::Normal(_) => {}
            _ => {
                return Err(WorkspaceSdkError::invalid_input(
                    "path must stay within workspace root",
                ));
            }
        }
    }

    let mut current = root.to_path_buf();
    let mut remaining = rel.components().peekable();
    while let Some(component) = remaining.peek().copied() {
        let Component::Normal(part) = component else {
            let _ = remaining.next();
            continue;
        };
        let candidate = current.join(part);
        if candidate.exists() {
            current = fs::canonicalize(&candidate).map_err(|error| {
                WorkspaceSdkError::io(format!("failed to resolve {}", candidate.display()), error)
            })?;
            if !current.starts_with(&root) {
                return Err(WorkspaceSdkError::invalid_input(
                    "path escapes workspace root",
                ));
            }
            let _ = remaining.next();
            continue;
        }
        break;
    }

    for component in remaining {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
    }

    if !current.starts_with(&root) {
        return Err(WorkspaceSdkError::invalid_input(
            "path escapes workspace root",
        ));
    }
    Ok(current)
}

pub fn workspace_relative_display(root: &Path, absolute: &Path) -> String {
    absolute
        .strip_prefix(root)
        .unwrap_or(absolute)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs as unix_fs};

    use super::resolve_workspace_path;

    #[test]
    fn rejects_absolute_and_parent_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        assert!(resolve_workspace_path(&root, "/tmp/nope").is_err());
        assert!(resolve_workspace_path(&root, "../nope").is_err());
    }

    #[test]
    fn rejects_symlink_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("root");
        fs::create_dir_all(&root).expect("mkdir");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        unix_fs::symlink(&outside, root.join("link")).expect("symlink");
        let root = root.canonicalize().expect("canon");
        assert!(resolve_workspace_path(&root, "link/secret.txt").is_err());
    }
}
