//! Bounded parsing of fetch and repository metadata.
//!
//! Only host-side bytes are parsed here: refs, tree records and object record
//! summaries. No Git object is imported, checked out, or executed on the host.

use runner_protocol::GitObjectFormat;

pub(crate) const MAX_TREE_PATH_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitFetchError {
    InvalidRemote,
    InvalidRef,
    HeadMismatch,
    TreeLimit,
    ObjectLimit,
    Malformed,
}

impl std::fmt::Display for GitFetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRemote => "git remote metadata is invalid",
            Self::InvalidRef => "git local ref is invalid",
            Self::HeadMismatch => "git fetched head changed",
            Self::TreeLimit => "git tree metadata exceeds its limit",
            Self::ObjectLimit => "git object metadata exceeds its limit",
            Self::Malformed => "git repository metadata is malformed",
        })
    }
}

impl std::error::Error for GitFetchError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TreeStats {
    pub(crate) entries: u64,
    pub(crate) total_bytes: u64,
    pub(crate) files: u64,
    pub(crate) symlinks: u64,
    pub(crate) gitlinks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObjectStats {
    pub(crate) objects: u64,
    pub(crate) total_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitFetchMetadata {
    pub(crate) head: String,
    pub(crate) object_format: GitObjectFormat,
    pub(crate) tree: TreeStats,
    pub(crate) objects: ObjectStats,
}

/// Parse exactly one `refs/heads/<branch>` line and infer the object format.
pub(crate) fn parse_ls_remote(
    stdout: &[u8],
    branch: &str,
) -> Result<(String, GitObjectFormat), GitFetchError> {
    let text = std::str::from_utf8(stdout).map_err(|_| GitFetchError::Malformed)?;
    let expected_ref = format!("refs/heads/{branch}");
    let mut found: Option<(String, GitObjectFormat)> = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Some((head, reference)) = line.split_once('\t') else {
            return Err(GitFetchError::InvalidRemote);
        };
        let format = match head.len() {
            40 if is_lower_hex(head) => GitObjectFormat::Sha1,
            64 if is_lower_hex(head) => GitObjectFormat::Sha256,
            _ => return Err(GitFetchError::InvalidRemote),
        };
        if reference.trim() != expected_ref || found.is_some() {
            return Err(GitFetchError::InvalidRemote);
        }
        found = Some((head.to_owned(), format));
    }
    found.ok_or(GitFetchError::InvalidRemote)
}

/// Require the local branch ref to equal the pinned remote head.
pub(crate) fn parse_local_ref(
    stdout: &[u8],
    branch: &str,
    expected_head: &str,
) -> Result<(), GitFetchError> {
    let text = std::str::from_utf8(stdout).map_err(|_| GitFetchError::Malformed)?;
    let local = text.trim();
    if local != expected_head || !is_lower_hex(local) {
        return Err(GitFetchError::HeadMismatch);
    }
    let _ = branch;
    Ok(())
}

/// Parse `git ls-tree -r -l -z` output.
///
/// Records are NUL-separated bytes; paths may contain newlines or non-UTF8
/// bytes. Modes are the closed object classes only.
pub(crate) fn parse_tree_records(
    bytes: &[u8],
    max_entries: u64,
    max_bytes: u64,
) -> Result<TreeStats, GitFetchError> {
    let mut stats = TreeStats {
        entries: 0,
        total_bytes: 0,
        files: 0,
        symlinks: 0,
        gitlinks: 0,
    };
    for record in bytes.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or(GitFetchError::Malformed)?;
        let (meta, path) = (&record[..tab], &record[tab + 1..]);
        if path.is_empty() || path.len() > MAX_TREE_PATH_BYTES {
            return Err(GitFetchError::TreeLimit);
        }
        let meta = std::str::from_utf8(meta).map_err(|_| GitFetchError::Malformed)?;
        let mut fields = meta.split_whitespace();
        let mode = fields.next().ok_or(GitFetchError::Malformed)?;
        let kind = fields.next().ok_or(GitFetchError::Malformed)?;
        let hash = fields.next().ok_or(GitFetchError::Malformed)?;
        let size = fields.next().ok_or(GitFetchError::Malformed)?;
        if fields.next().is_some() || !is_lower_hex(hash) {
            return Err(GitFetchError::Malformed);
        }
        stats.entries = stats
            .entries
            .checked_add(1)
            .ok_or(GitFetchError::TreeLimit)?;
        match (mode, kind) {
            ("100644" | "100755", "blob") => {
                let size: u64 = size.parse().map_err(|_| GitFetchError::Malformed)?;
                stats.files += 1;
                stats.total_bytes = stats
                    .total_bytes
                    .checked_add(size)
                    .ok_or(GitFetchError::TreeLimit)?;
            }
            ("120000", "blob") => {
                stats.symlinks += 1;
            }
            ("160000", "commit") => {
                stats.gitlinks += 1;
            }
            _ => return Err(GitFetchError::Malformed),
        }
        if stats.entries > max_entries || stats.total_bytes > max_bytes {
            return Err(GitFetchError::TreeLimit);
        }
    }
    Ok(stats)
}

/// Parse `git cat-file --batch-all-objects --batch-check` records.
pub(crate) fn parse_object_batch(
    bytes: &[u8],
    max_objects: u64,
    max_bytes: u64,
) -> Result<ObjectStats, GitFetchError> {
    let text = std::str::from_utf8(bytes).map_err(|_| GitFetchError::Malformed)?;
    let mut stats = ObjectStats {
        objects: 0,
        total_bytes: 0,
    };
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let hash = fields.next().ok_or(GitFetchError::Malformed)?;
        let _kind = fields.next().ok_or(GitFetchError::Malformed)?;
        let size = fields.next().ok_or(GitFetchError::Malformed)?;
        if fields.next().is_some() || !is_lower_hex(hash) {
            return Err(GitFetchError::Malformed);
        }
        let size: u64 = size.parse().map_err(|_| GitFetchError::Malformed)?;
        stats.objects = stats
            .objects
            .checked_add(1)
            .ok_or(GitFetchError::ObjectLimit)?;
        stats.total_bytes = stats
            .total_bytes
            .checked_add(size)
            .ok_or(GitFetchError::ObjectLimit)?;
        if stats.objects > max_objects || stats.total_bytes > max_bytes {
            return Err(GitFetchError::ObjectLimit);
        }
    }
    Ok(stats)
}

fn is_lower_hex(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_remote_requires_one_exact_head_and_infers_format() {
        let sha1 = "a".repeat(40);
        let line = format!("{sha1}\trefs/heads/main\n");
        assert_eq!(
            parse_ls_remote(line.as_bytes(), "main").expect("sha1"),
            (sha1.clone(), GitObjectFormat::Sha1)
        );
        let sha256 = "b".repeat(64);
        let line = format!("{sha256}\trefs/heads/main\n");
        assert_eq!(
            parse_ls_remote(line.as_bytes(), "main").expect("sha256"),
            (sha256, GitObjectFormat::Sha256)
        );
        assert!(parse_ls_remote(b"", "main").is_err());
        assert!(parse_ls_remote(format!("{sha1}\trefs/heads/other\n").as_bytes(), "main").is_err());
        assert!(
            parse_ls_remote(
                format!("{sha1}\trefs/heads/main\n{sha1}\trefs/heads/main\n").as_bytes(),
                "main"
            )
            .is_err()
        );
    }

    #[test]
    fn changed_local_head_is_a_conflict() {
        let head = "a".repeat(40);
        assert!(parse_local_ref(format!("{head}\n").as_bytes(), "main", &head).is_ok());
        assert!(
            parse_local_ref(format!("{}\n", "b".repeat(40)).as_bytes(), "main", &head).is_err()
        );
    }

    #[test]
    fn tree_records_are_nul_parsed_and_counted_separately() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"100644 blob ");
        bytes.extend_from_slice("a".repeat(40).as_bytes());
        bytes.extend_from_slice(b" 12\tpath with space\n");
        bytes.push(0);
        bytes.extend_from_slice(b"100755 blob ");
        bytes.extend_from_slice("b".repeat(40).as_bytes());
        bytes.extend_from_slice(b" 3\texec.sh");
        bytes.push(0);
        bytes.extend_from_slice(b"120000 blob ");
        bytes.extend_from_slice("c".repeat(40).as_bytes());
        bytes.extend_from_slice(b" 5\tlink");
        bytes.push(0);
        bytes.extend_from_slice(b"160000 commit ");
        bytes.extend_from_slice("d".repeat(40).as_bytes());
        bytes.extend_from_slice(b" -\tsubmodule");
        bytes.push(0);
        let stats = parse_tree_records(&bytes, 10, 1000).expect("stats");
        assert_eq!(stats.entries, 4);
        assert_eq!(stats.files, 2);
        assert_eq!(stats.symlinks, 1);
        assert_eq!(stats.gitlinks, 1);
        assert_eq!(stats.total_bytes, 15);
        assert_eq!(
            parse_tree_records(&bytes, 3, 1000),
            Err(GitFetchError::TreeLimit)
        );
        assert_eq!(
            parse_tree_records(b"100644 blob x 1\tbad", 10, 1000),
            Err(GitFetchError::Malformed)
        );
    }

    #[test]
    fn object_batch_sums_all_logical_sizes_with_caps() {
        let bytes = format!("{} blob 10\n{} blob 20\n", "a".repeat(40), "b".repeat(40));
        let stats = parse_object_batch(bytes.as_bytes(), 2, 30).expect("objects");
        assert_eq!(stats.objects, 2);
        assert_eq!(stats.total_bytes, 30);
        assert_eq!(
            parse_object_batch(bytes.as_bytes(), 2, 29),
            Err(GitFetchError::ObjectLimit)
        );
        assert_eq!(
            parse_object_batch(bytes.as_bytes(), 1, 100),
            Err(GitFetchError::ObjectLimit)
        );
    }
}
