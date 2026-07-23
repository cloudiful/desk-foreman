use std::path::Path;

use crate::{
    ApplyPatchChange, ApplyPatchOperation, ApplyPatchStatus, ApplyPatchSummary, WorkspaceSdkError,
    patch::{
        commit::commit_change,
        parser::parse_patch,
        plan::{PlannedChange, plan_patch},
    },
};

pub(crate) fn apply_patch(
    root: &Path,
    patch: &str,
) -> Result<ApplyPatchSummary, WorkspaceSdkError> {
    let parsed = parse_patch(patch)?;
    let planned = plan_patch(root, &parsed)?;
    commit_planned(root, &planned)
}

pub(crate) fn commit_planned(
    root: &Path,
    planned: &[PlannedChange],
) -> Result<ApplyPatchSummary, WorkspaceSdkError> {
    let mut changes = planned.iter().map(change_pending).collect::<Vec<_>>();
    for (index, planned_change) in planned.iter().enumerate() {
        match commit_change(root, planned_change) {
            Ok(()) => changes[index].status = ApplyPatchStatus::Applied,
            Err(error) => {
                changes[index].status = ApplyPatchStatus::Failed;
                changes[index].error = Some(error);
                for change in changes.iter_mut().skip(index + 1) {
                    change.status = ApplyPatchStatus::Skipped;
                    change.error =
                        Some("not attempted after an earlier commit failure".to_string());
                }
                return Ok(summary(changes, true));
            }
        }
    }
    Ok(summary(changes, false))
}

fn change_pending(change: &PlannedChange) -> ApplyPatchChange {
    ApplyPatchChange {
        operation: change.operation,
        path: change.path.clone(),
        destination: change.destination.clone(),
        status: ApplyPatchStatus::Skipped,
        error: None,
        added_lines: change.added_lines,
        deleted_lines: change.deleted_lines,
    }
}

fn summary(changes: Vec<ApplyPatchChange>, partial: bool) -> ApplyPatchSummary {
    let summary = changes
        .iter()
        .map(summary_line)
        .collect::<Vec<_>>()
        .join("\n");
    ApplyPatchSummary {
        summary,
        partial,
        changes,
    }
}

fn summary_line(change: &ApplyPatchChange) -> String {
    let operation = match change.operation {
        ApplyPatchOperation::Add => "A",
        ApplyPatchOperation::Delete => "D",
        ApplyPatchOperation::Update => "U",
        ApplyPatchOperation::Move => "M",
    };
    let target = change
        .destination
        .as_deref()
        .map(|destination| format!(" -> {destination}"))
        .unwrap_or_default();
    let suffix = match (&change.status, &change.error) {
        (ApplyPatchStatus::Applied, _) => String::new(),
        (ApplyPatchStatus::Failed, Some(error)) => format!(" (failed: {error})"),
        (ApplyPatchStatus::Skipped, Some(error)) => format!(" (skipped: {error})"),
        _ => String::new(),
    };
    format!("{operation} {}{target}{suffix}", change.path)
}
