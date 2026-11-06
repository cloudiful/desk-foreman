use super::import::GitImportError;

/// Limits for staging one raw Git object snapshot.
///
/// Every limit is required and must be explicitly positive. There is no
/// default or production value in this phase; a future caller decides the
/// budgets before any runtime wiring exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GitImportLimits {
    /// Every directory entry enumerated under `.git/objects`, including
    /// directories.
    pub(crate) max_entries: u64,
    /// Every file written into the quarantine snapshot, including HEAD and
    /// the pinned branch ref.
    pub(crate) max_files: u64,
    /// Maximum raw bytes for one object file.
    pub(crate) max_file_bytes: u64,
    /// Maximum raw bytes across all copied object files.
    pub(crate) max_total_bytes: u64,
    /// Maximum raw bytes across the copied HEAD and pinned branch ref.
    pub(crate) max_metadata_bytes: u64,
}

const MAX_LIMIT: u64 = i64::MAX as u64;

impl GitImportLimits {
    pub(crate) fn validate(self) -> Result<Self, GitImportError> {
        for limit in [
            self.max_entries,
            self.max_files,
            self.max_file_bytes,
            self.max_total_bytes,
            self.max_metadata_bytes,
        ] {
            if limit == 0 || limit > MAX_LIMIT {
                return Err(GitImportError::InvalidLimits);
            }
        }
        Ok(self)
    }
}
