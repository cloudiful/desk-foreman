//! Effective-uid discovery for host-file ownership checks
//! (issue 749 P2a repair).
//!
//! A credential file is only trustworthy because it is owned by the manager
//! user. Proving that needs the process's *effective* uid, which is what the
//! kernel uses for permission checks — not the real uid, which can differ
//! for a setuid binary and would silently widen the set of files accepted.
//!
//! There is deliberately no fallback. If the effective uid cannot be
//! established the caller must fail closed, because "could not check the
//! owner" and "the owner is correct" are not the same answer, and guessing
//! the second one turns an unavailable check into a silent pass.

use std::fs;

/// procfs entry carrying the process credential set.
const PROC_STATUS: &str = "/proc/self/status";

/// Where the effective uid comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UidSource {
    /// Read the effective uid from procfs, failing closed if unavailable.
    /// The only variant production can use.
    Proc,
    /// A known uid, for tests that must not depend on the host account.
    #[cfg(test)]
    Fixed(u32),
    /// Simulate procfs being absent or unreadable.
    #[cfg(test)]
    Unavailable,
}

impl UidSource {
    /// Resolve the effective uid, or explain why ownership cannot be
    /// established.
    pub(crate) fn effective_uid(self) -> anyhow::Result<u32> {
        match self {
            Self::Proc => read_effective_uid(PROC_STATUS),
            #[cfg(test)]
            Self::Fixed(uid) => Ok(uid),
            #[cfg(test)]
            Self::Unavailable => {
                anyhow::bail!("effective uid is unavailable; refusing to check file ownership")
            }
        }
    }
}

/// Read the effective uid from a procfs `status` file.
///
/// The `Uid:` line lists real, effective, saved-set, and filesystem uids; the
/// second field is the effective one. A file without that line, or with a
/// field that is not a number, is an error rather than a skip.
pub(crate) fn read_effective_uid(status_path: &str) -> anyhow::Result<u32> {
    let status = fs::read_to_string(status_path).map_err(|error| {
        anyhow::anyhow!("effective uid is unavailable ({status_path}): {error}")
    })?;
    parse_effective_uid(&status)
}

/// Pure extraction, split out so the field selection is testable without
/// depending on the host's procfs.
pub(crate) fn parse_effective_uid(status: &str) -> anyhow::Result<u32> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or_else(|| {
            anyhow::anyhow!("effective uid is unavailable: procfs reported no Uid line")
        })?;
    let mut fields = line.split_whitespace();
    let _real = fields.next();
    let effective = fields.next().ok_or_else(|| {
        anyhow::anyhow!("effective uid is unavailable: procfs Uid line has no effective field")
    })?;
    effective.parse::<u32>().map_err(|error| {
        anyhow::anyhow!("effective uid is unavailable: Uid line is not numeric ({error})")
    })
}
