use std::path::Path;

use crate::{
    ApplyPatchStatus, PatchApplied, PatchChange, PatchChangeError, PatchEffect, PatchIncomplete,
    PatchResult, PatchWrites,
    patch::{
        commit::{FsOps, commit_change, commit_diagnostic},
        parser::parse_patch,
        plan::{PlannedChange, plan_patch},
    },
};

pub(crate) fn apply_patch(root: &Path, patch: &str) -> PatchResult {
    let parsed = match parse_patch(patch) {
        Ok(parsed) => parsed,
        Err(diagnostic) => return PatchResult::Rejected(crate::PatchRejected { diagnostic }),
    };
    let planned = match plan_patch(root, &parsed) {
        Ok(planned) => planned,
        Err(diagnostic) => return PatchResult::Rejected(crate::PatchRejected { diagnostic }),
    };
    commit_planned(root, &planned)
}

pub(crate) fn commit_planned(root: &Path, planned: &[PlannedChange]) -> PatchResult {
    commit_planned_impl(root, planned, FsOps::REAL)
}

#[cfg(test)]
pub(crate) fn commit_planned_with(
    root: &Path,
    planned: &[PlannedChange],
    ops: FsOps,
) -> PatchResult {
    commit_planned_impl(root, planned, ops)
}

fn commit_planned_impl(root: &Path, planned: &[PlannedChange], ops: FsOps) -> PatchResult {
    let mut changes = Vec::with_capacity(planned.len());
    for (index, planned_change) in planned.iter().enumerate() {
        let outcome = commit_change(root, planned_change, ops);
        match outcome.error {
            None => changes.push(PatchChange {
                operation: planned_change.operation,
                path: planned_change.path.clone(),
                destination: planned_change.destination.clone(),
                status: ApplyPatchStatus::Applied,
                effect: outcome.effect,
                error: None,
            }),
            Some(code) => {
                changes.push(PatchChange {
                    operation: planned_change.operation,
                    path: planned_change.path.clone(),
                    destination: planned_change.destination.clone(),
                    status: ApplyPatchStatus::Failed,
                    effect: outcome.effect,
                    error: Some(PatchChangeError::new(code)),
                });
                for skipped in planned.iter().skip(index + 1) {
                    changes.push(PatchChange {
                        operation: skipped.operation,
                        path: skipped.path.clone(),
                        destination: skipped.destination.clone(),
                        status: ApplyPatchStatus::Skipped,
                        effect: PatchEffect::Unchanged,
                        error: None,
                    });
                }
                return PatchResult::Incomplete(PatchIncomplete {
                    writes: classify_writes(&changes),
                    changes,
                    diagnostic: commit_diagnostic(planned_change, code),
                });
            }
        }
    }
    PatchResult::Applied(PatchApplied { changes })
}

pub fn classify_writes(changes: &[PatchChange]) -> PatchWrites {
    if changes
        .iter()
        .any(|change| change.effect == PatchEffect::Unknown)
    {
        return PatchWrites::Unknown;
    }
    let any_changed = changes
        .iter()
        .any(|change| change.effect == PatchEffect::Changed);
    if !any_changed {
        return PatchWrites::None;
    }
    if changes
        .iter()
        .all(|change| change.status == ApplyPatchStatus::Applied)
    {
        PatchWrites::All
    } else {
        PatchWrites::Partial
    }
}
