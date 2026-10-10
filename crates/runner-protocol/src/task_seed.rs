//! Shared immutable task-input seed rules (issue 753 P2b).
//!
//! Moved here from the control plane so the API side and the runner manager
//! validate, order and hash a seed with exactly one implementation. The digest
//! is a content manifest: it is a pure function of the file bytes, so it is
//! independent of JSON whitespace, request order, task id and generation. That
//! property is what lets the same bytes be compared across the control plane,
//! the manager and a later canonical-project CAS.
//!
//! Nothing here reads or writes a filesystem, database or network: it is pure
//! rule and byte encoding.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[cfg(test)]
mod tests;

/// Largest accepted snapshot entry count.
pub const MAX_SEED_ENTRIES: usize = 4096;
/// Largest accepted single path, in UTF-8 bytes.
pub const MAX_SEED_PATH_BYTES: usize = 512;
/// Largest accepted single file content, in UTF-8 bytes.
pub const MAX_SEED_CONTENT_BYTES: usize = 1024 * 1024;
/// Largest accepted aggregate content, in UTF-8 bytes.
pub const MAX_SEED_TOTAL_BYTES: usize = 8 * 1024 * 1024;

/// Versioned manifest domain separator, including its terminating NUL.
pub const SEED_MANIFEST_PREFIX: &[u8] = b"df-native-task-seed-v1\0";

/// One authorized seed file as it crosses the wire.
///
/// `deny_unknown_fields` at this level and in the enclosing request means a
/// caller cannot smuggle a mode, a symlink target or a host path alongside the
/// path and content.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceSeedSnapshotFile {
    pub path: String,
    pub content: String,
}

impl SourceSeedSnapshotFile {
    /// Build one file entry.
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// Why a seed bundle was refused.
///
/// Each variant maps to exactly one status on the API side: shape rules are
/// `400`, size ceilings are `413`. Keeping the split in the type means no
/// caller makes its own judgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedBundleError {
    /// The path is not a normalized relative POSIX path, or names `.git`.
    InvalidPath,
    /// The path is longer than [`MAX_SEED_PATH_BYTES`].
    PathTooLong,
    /// Two entries name the same path.
    DuplicatePath,
    /// One path is also an ancestor directory of another.
    AncestorCollision,
    /// The content contains a NUL, which a `TEXT` column cannot store.
    ContentNotSupported,
    /// More entries than [`MAX_SEED_ENTRIES`].
    TooManyEntries,
    /// One content is longer than [`MAX_SEED_CONTENT_BYTES`].
    ContentTooLarge,
    /// The aggregate content is longer than [`MAX_SEED_TOTAL_BYTES`].
    TotalTooLarge,
}

impl SeedBundleError {
    /// Stable, prose-free code for assertions and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath => "invalid_path",
            Self::PathTooLong => "path_too_long",
            Self::DuplicatePath => "duplicate_path",
            Self::AncestorCollision => "ancestor_collision",
            Self::ContentNotSupported => "content_not_supported",
            Self::TooManyEntries => "too_many_entries",
            Self::ContentTooLarge => "content_too_large",
            Self::TotalTooLarge => "total_too_large",
        }
    }
}

/// A validated, canonically ordered seed bundle.
///
/// Constructed only by [`validate_seed_bundle`], so holding one is proof the
/// limits and path rules already hold and the files are in path order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedBundle {
    /// Files in lexicographic path order.
    pub files: Vec<SourceSeedSnapshotFile>,
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
}

/// Validate, order and hash one seed bundle.
///
/// Returns the canonical bundle or the first rule that failed. The caller maps
/// the error to a status; no status decision happens here.
pub fn validate_seed_bundle(
    files: &[SourceSeedSnapshotFile],
) -> Result<SeedBundle, SeedBundleError> {
    if files.len() > MAX_SEED_ENTRIES {
        return Err(SeedBundleError::TooManyEntries);
    }

    let mut total_bytes: usize = 0;
    for file in files {
        if file.path.len() > MAX_SEED_PATH_BYTES {
            return Err(SeedBundleError::PathTooLong);
        }
        if !is_normalized_relative_posix(&file.path) {
            return Err(SeedBundleError::InvalidPath);
        }
        if file.content.contains('\0') {
            // A NUL cannot be stored in a SQL text column, so it is refused
            // before any statement runs.
            return Err(SeedBundleError::ContentNotSupported);
        }
        if file.content.len() > MAX_SEED_CONTENT_BYTES {
            return Err(SeedBundleError::ContentTooLarge);
        }
        total_bytes = total_bytes.saturating_add(file.content.len());
        if total_bytes > MAX_SEED_TOTAL_BYTES {
            return Err(SeedBundleError::TotalTooLarge);
        }
    }

    let mut files: Vec<SourceSeedSnapshotFile> = files.to_vec();
    files.sort_by(|left, right| left.path.cmp(&right.path));

    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(SeedBundleError::DuplicatePath);
    }
    if has_ancestor_collision(&files) {
        return Err(SeedBundleError::AncestorCollision);
    }

    let manifest_sha256 = manifest_sha256(&files);
    Ok(SeedBundle {
        file_count: u32::try_from(files.len()).unwrap_or_default(),
        total_bytes: u32::try_from(total_bytes).unwrap_or_default(),
        files,
        manifest_sha256,
    })
}

/// True when `path` is a normalized relative POSIX path with no `.git` part.
///
/// Rejected: absolute paths, Windows drive prefixes, backslashes, NUL and
/// other control characters (including DEL), an empty leading/trailing or
/// interior component, `.` and `..` components, and any component named `.git`
/// in any case.
pub fn is_normalized_relative_posix(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    if path.contains('\\') {
        return false;
    }
    if path.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return false;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return false;
        }
        if component.eq_ignore_ascii_case(".git") {
            return false;
        }
    }
    true
}

/// True when one path is a directory ancestor of another.
///
/// `a` and `a/b` collide: a component cannot be both a file and a directory.
pub fn has_ancestor_collision(files: &[SourceSeedSnapshotFile]) -> bool {
    let paths: std::collections::HashSet<&str> =
        files.iter().map(|file| file.path.as_str()).collect();
    for file in files {
        let mut end = 0;
        while let Some(offset) = file.path[end..].find('/') {
            end += offset;
            if paths.contains(&file.path[..end]) {
                return true;
            }
            end += 1;
        }
    }
    false
}

/// The versioned SHA-256 manifest of `files` (already in path order).
///
/// Fixed encoding, `df-native-task-seed-v1`:
///
/// ```text
/// "df-native-task-seed-v1" NUL
/// for each file in lexicographic path order:
///     path_byte_length: u64 big-endian
///     path bytes
///     content_byte_length: u64 big-endian
///     content bytes
/// ```
///
/// The length prefixes make the encoding unambiguous, so no two distinct file
/// lists can collide by concatenation.
pub fn manifest_sha256(files: &[SourceSeedSnapshotFile]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(SEED_MANIFEST_PREFIX);
    for file in files {
        hasher.update((file.path.len() as u64).to_be_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update((file.content.len() as u64).to_be_bytes());
        hasher.update(file.content.as_bytes());
    }
    lower_hex(&hasher.finalize())
}

/// Lower-case hex, matching the database check constraint.
fn lower_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
