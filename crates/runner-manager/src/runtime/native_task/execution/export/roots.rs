use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct ProtectedRoots(Vec<(PathBuf, Metadata)>);

impl ProtectedRoots {
    pub(super) fn capture(tree: &Path, uid: u32) -> anyhow::Result<Self> {
        let mut roots = Vec::new();
        let mut current = tree;
        // tree → task → environment → configured private manager root.
        for _ in 0..4 {
            super::super::super::paths::validate_private_dir(current, uid)?;
            let metadata = fs::symlink_metadata(current)?;
            roots.push((current.to_path_buf(), metadata));
            current = current
                .parent()
                .ok_or_else(|| anyhow::anyhow!("invalid export root"))?;
        }
        Ok(Self(roots))
    }

    pub(super) fn verify(&self, uid: u32) -> anyhow::Result<()> {
        for (path, before) in &self.0 {
            super::super::super::paths::validate_private_dir(path, uid)?;
            let after = fs::symlink_metadata(path)?;
            if before.dev() != after.dev() || before.ino() != after.ino() {
                anyhow::bail!("export protected root changed");
            }
        }
        Ok(())
    }
}
