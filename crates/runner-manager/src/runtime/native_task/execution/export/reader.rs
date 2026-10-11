use std::fs::{self, File, Metadata};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::bail;
use runner_protocol::MAX_SEED_CONTENT_BYTES;

fn same_identity(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.uid() == b.uid()
        && a.nlink() == b.nlink()
}

fn stable(a: &Metadata, b: &Metadata) -> bool {
    same_identity(a, b)
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
}

fn directory(metadata: &Metadata, uid: u32) -> anyhow::Result<()> {
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        bail!("unsafe selected export directory");
    }
    Ok(())
}

/// The caller holds the stopped task locks and validates the protected tree.
/// This check/open reader is not an openat-based race-free sandbox.
pub(super) fn selected_text(tree: &Path, path: &str, uid: u32) -> anyhow::Result<Option<String>> {
    let selection = runner_protocol::validate_native_task_export_selection(&[path.to_owned()])
        .map_err(|_| anyhow::anyhow!("invalid selected export path"))?;
    let mut current = tree.to_path_buf();
    let components: Vec<_> = selection.paths[0].split('/').collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let before = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => bail!("selected export entry is unavailable"),
        };
        if index + 1 != components.len() {
            directory(&before, uid)?;
            continue;
        }
        if !before.is_file()
            || before.uid() != uid
            || before.nlink() != 1
            || before.mode() & 0o022 != 0
        {
            bail!("unsafe selected export file");
        }
        if before.len() > MAX_SEED_CONTENT_BYTES as u64 {
            bail!("selected export file exceeds size limit");
        }
        let mut file = File::open(&current)
            .map_err(|_| anyhow::anyhow!("selected export file cannot be opened"))?;
        let opened = file
            .metadata()
            .map_err(|_| anyhow::anyhow!("selected export handle is unavailable"))?;
        if !stable(&before, &opened) {
            bail!("selected export file changed before read");
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_SEED_CONTENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow::anyhow!("selected export read failed"))?;
        let after = file
            .metadata()
            .map_err(|_| anyhow::anyhow!("selected export handle disappeared"))?;
        let named = fs::symlink_metadata(&current)
            .map_err(|_| anyhow::anyhow!("selected export entry disappeared"))?;
        if bytes.len() > MAX_SEED_CONTENT_BYTES
            || bytes.len() as u64 != before.len()
            || !stable(&opened, &after)
            || !stable(&after, &named)
        {
            bail!("selected export file changed during read");
        }
        if bytes.contains(&0) {
            bail!("selected export file contains NUL");
        }
        return String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| anyhow::anyhow!("selected export file is not UTF-8"));
    }
    bail!("invalid selected export path")
}
