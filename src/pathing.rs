use std::path::{Path, PathBuf};

pub fn resolve_workspace_path(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    desk_foreman_workspace_sdk::resolve_workspace_path(root, relative).map_err(anyhow::Error::from)
}

pub fn workspace_relative_display(root: &Path, absolute: &Path) -> String {
    desk_foreman_workspace_sdk::workspace_relative_display(root, absolute)
}

#[cfg(test)]
mod tests {
    use super::resolve_workspace_path;
    use std::{fs, os::unix::fs as unix_fs};

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
