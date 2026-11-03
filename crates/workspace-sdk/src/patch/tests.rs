use std::{fs, path::Path};

#[cfg(unix)]
use std::os::unix::fs as unix_fs;

use crate::{
    ApplyPatchOperation, ApplyPatchStatus, PatchApplied, PatchCode, PatchDiagnostic, PatchEffect,
    PatchIncomplete, PatchResult, PatchStage, PatchWrites, parse_patch,
};

use super::{
    apply::{apply_patch, commit_planned, commit_planned_with},
    commit::{FsOps, commit_change, real_remove, real_rename},
    plan::plan_patch,
};

fn failing_rename(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("injected rename failure"))
}

fn failing_remove(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("injected remove failure"))
}

fn expect_applied(result: PatchResult) -> PatchApplied {
    match result {
        PatchResult::Applied(applied) => applied,
        other => panic!("expected applied, got {other:?}"),
    }
}

fn expect_rejected(result: PatchResult) -> PatchDiagnostic {
    match result {
        PatchResult::Rejected(rejected) => rejected.diagnostic,
        other => panic!("expected rejected, got {other:?}"),
    }
}

fn expect_incomplete(result: PatchResult) -> PatchIncomplete {
    match result {
        PatchResult::Incomplete(incomplete) => incomplete,
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn parses_and_applies_add_update_delete_and_move() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    fs::write(root.join("hello.txt"), "hello\nworld\n").expect("seed");
    fs::write(root.join("delete.txt"), "gone\n").expect("seed");

    let patch = "\
*** Begin Patch
*** Update File: hello.txt
*** Move to: moved.txt
@@
 hello
-world
+desk foreman
*** Add File: new.txt
+fresh
*** Delete File: delete.txt
*** End Patch";

    let applied = expect_applied(apply_patch(root, patch));
    assert_eq!(applied.changes.len(), 3);
    let moved = &applied.changes[0];
    assert_eq!(moved.status, ApplyPatchStatus::Applied);
    assert_eq!(moved.effect, PatchEffect::Changed);
    assert_eq!(moved.destination.as_deref(), Some("moved.txt"));
    assert!(moved.error.is_none());
    assert_eq!(applied.changes[1].effect, PatchEffect::Changed);
    assert!(applied.changes[1].error.is_none());
    assert_eq!(
        fs::read_to_string(root.join("moved.txt")).expect("moved file"),
        "hello\ndesk foreman\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("new.txt")).expect("new file"),
        "fresh\n"
    );
    assert!(!root.join("hello.txt").exists());
    assert!(!root.join("delete.txt").exists());
}

#[test]
fn rejects_empty_and_bad_grammar() {
    let diagnostic = expect_rejected(apply_patch(
        std::path::Path::new("."),
        "*** Begin Patch\n*** End Patch\n",
    ));
    assert_eq!(diagnostic.code, PatchCode::PatchArgInvalid);
    assert_eq!(diagnostic.stage, PatchStage::Params);

    let diagnostic = expect_rejected(apply_patch(std::path::Path::new("."), "bad"));
    assert_eq!(diagnostic.code, PatchCode::PatchBoundaryInvalid);
    assert_eq!(diagnostic.stage, PatchStage::Parse);

    let diagnostic = expect_rejected(apply_patch(
        std::path::Path::new("."),
        "*** Begin Patch\n*** Add File: x\n*** End Patch",
    ));
    assert_eq!(diagnostic.code, PatchCode::PatchHunkInvalid);
}

#[test]
fn rejects_absolute_paths_and_conflicting_operations_without_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = "\
*** Begin Patch
*** Add File: a.txt
+one
*** Add File: a.txt
+two
*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), patch));
    assert_eq!(diagnostic.code, PatchCode::PatchDuplicateTarget);
    assert_eq!(diagnostic.path.as_deref(), Some("a.txt"));
    assert!(!temp.path().join("a.txt").exists());

    let absolute = "*** Begin Patch\n*** Add File: /tmp/nope\n+x\n*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), absolute));
    assert_eq!(diagnostic.code, PatchCode::PatchPathInvalid);
    assert!(diagnostic.path.is_none());
    assert_eq!(
        diagnostic.message(),
        "patch path is not a valid workspace-relative path"
    );
    assert!(!diagnostic.message().contains("/tmp"));
}

#[test]
fn applies_whitespace_and_unicode_punctuation_fallbacks() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "  println!(\"hello\");  \n").expect("seed");
    let patch = "\
*** Begin Patch
*** Update File: a.txt
@@
-println!(“hello”);
+println!(‘world’);
*** End Patch";

    let applied = expect_applied(apply_patch(temp.path(), patch));
    assert_eq!(applied.changes[0].status, ApplyPatchStatus::Applied);
    assert_eq!(
        fs::read_to_string(temp.path().join("a.txt")).expect("updated"),
        "println!(‘world’);\n"
    );
}

#[test]
fn rejects_ambiguous_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "same\nsame\n").expect("seed");
    let patch = "\
*** Begin Patch
*** Update File: a.txt
@@
-same
+changed
*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), patch));
    assert_eq!(diagnostic.code, PatchCode::PatchContextAmbiguous);
    assert_eq!(diagnostic.path.as_deref(), Some("a.txt"));
    assert_eq!(diagnostic.hunk, Some(0));
    assert_eq!(
        fs::read_to_string(temp.path().join("a.txt")).unwrap(),
        "same\nsame\n"
    );
}

#[test]
fn eof_hunk_only_matches_end_of_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "one\ntwo\n").expect("seed");
    let patch = "\
*** Begin Patch
*** Update File: a.txt
@@
 two
+three
*** End of File
*** End Patch";
    expect_applied(apply_patch(temp.path(), patch));
    assert_eq!(
        fs::read_to_string(temp.path().join("a.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[test]
fn applies_hunk_to_a_file_containing_one_blank_line() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("blank.txt"), "\n").expect("seed");
    let patch = "\
*** Begin Patch
*** Update File: blank.txt
@@
-
+filled
*** End Patch";

    expect_applied(apply_patch(temp.path(), patch));
    assert_eq!(
        fs::read_to_string(temp.path().join("blank.txt")).unwrap(),
        "filled\n"
    );
}

#[test]
fn preserves_crlf_and_bom() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), b"\xef\xbb\xbfone\r\ntwo\r\n").expect("seed");
    let patch = "\
*** Begin Patch
*** Update File: a.txt
@@
-two
+three
*** End Patch";
    expect_applied(apply_patch(temp.path(), patch));
    assert_eq!(
        fs::read(temp.path().join("a.txt")).unwrap(),
        b"\xef\xbb\xbfone\r\nthree\r\n"
    );
}

#[test]
fn identical_update_is_applied_with_unchanged_effect() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "same\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-same\n+same\n*** End Patch";
    let applied = expect_applied(apply_patch(temp.path(), patch));
    assert_eq!(applied.changes[0].status, ApplyPatchStatus::Applied);
    assert_eq!(applied.changes[0].effect, PatchEffect::Unchanged);
}

#[test]
fn delete_accepts_non_utf8_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("binary.bin"), [0, 159, 146, 150]).expect("seed");
    let patch = "*** Begin Patch\n*** Delete File: binary.bin\n*** End Patch";
    let applied = expect_applied(apply_patch(temp.path(), patch));
    assert_eq!(applied.changes[0].effect, PatchEffect::Changed);
    assert!(!temp.path().join("binary.bin").exists());
}

#[test]
fn preserves_applied_changes_and_reports_commit_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("first.txt"), "one\n").expect("seed");
    fs::write(temp.path().join("second.txt"), "two\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: first.txt\n@@\n-one\n+updated\n*** Update File: second.txt\n@@\n-two\n+changed\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");
    fs::write(temp.path().join("second.txt"), "external\n").expect("external edit");

    let incomplete = expect_incomplete(commit_planned(temp.path(), &planned));
    assert_eq!(incomplete.writes, PatchWrites::Partial);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Applied);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Changed);
    assert_eq!(incomplete.changes[1].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[1].effect, PatchEffect::Unchanged);
    assert_eq!(
        incomplete.changes[1].error.as_ref().map(|error| error.code),
        Some(PatchCode::PatchTargetChanged)
    );
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchTargetChanged);
    assert_eq!(incomplete.diagnostic.path.as_deref(), Some("second.txt"));
    assert_eq!(
        fs::read_to_string(temp.path().join("first.txt")).unwrap(),
        "updated\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("second.txt")).unwrap(),
        "external\n"
    );
}

#[test]
fn first_commit_failure_leaves_no_effects_and_skips_the_rest() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("first.txt"), "one\n").expect("seed");
    fs::write(temp.path().join("second.txt"), "two\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: first.txt\n@@\n-one\n+updated\n*** Update File: second.txt\n@@\n-two\n+changed\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");
    fs::write(temp.path().join("first.txt"), "external\n").expect("external edit");

    let incomplete = expect_incomplete(commit_planned(temp.path(), &planned));
    assert_eq!(incomplete.writes, PatchWrites::None);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unchanged);
    assert_eq!(incomplete.changes[1].status, ApplyPatchStatus::Skipped);
    assert_eq!(incomplete.changes[1].effect, PatchEffect::Unchanged);
    assert!(incomplete.changes[1].error.is_none());
    assert_eq!(
        fs::read_to_string(temp.path().join("first.txt")).unwrap(),
        "external\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("second.txt")).unwrap(),
        "two\n"
    );
}

#[test]
fn second_commit_failure_keeps_the_applied_prefix_and_skips_the_tail() {
    let temp = tempfile::tempdir().expect("tempdir");
    for (name, content) in [
        ("first.txt", "one\n"),
        ("second.txt", "two\n"),
        ("third.txt", "three\n"),
    ] {
        fs::write(temp.path().join(name), content).expect("seed");
    }
    let patch = "*** Begin Patch\n*** Update File: first.txt\n@@\n-one\n+updated\n*** Update File: second.txt\n@@\n-two\n+changed\n*** Update File: third.txt\n@@\n-three\n+later\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");
    fs::write(temp.path().join("second.txt"), "external\n").expect("external edit");

    let incomplete = expect_incomplete(commit_planned(temp.path(), &planned));
    assert_eq!(incomplete.writes, PatchWrites::Partial);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Applied);
    assert_eq!(incomplete.changes[1].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[2].status, ApplyPatchStatus::Skipped);
    assert_eq!(incomplete.changes[2].effect, PatchEffect::Unchanged);
    assert!(incomplete.changes[2].error.is_none());
    assert_eq!(
        fs::read_to_string(temp.path().join("third.txt")).unwrap(),
        "three\n"
    );
}

#[test]
fn move_source_snapshot_change_reports_patch_source_changed() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("source")).expect("source dir");
    fs::create_dir(temp.path().join("dest")).expect("dest dir");
    fs::write(temp.path().join("source/file.txt"), "one\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: source/file.txt\n*** Move to: dest/target.txt\n@@\n-one\n+two\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    fs::write(temp.path().join("source/file.txt"), "changed\n").expect("change source snapshot");

    let incomplete = expect_incomplete(commit_planned(temp.path(), &planned));
    assert_eq!(incomplete.writes, PatchWrites::None);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unchanged);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchSourceChanged);
    assert_eq!(incomplete.diagnostic.stage, PatchStage::Commit);
    assert_eq!(
        incomplete.diagnostic.path.as_deref(),
        Some("source/file.txt")
    );
    assert!(!temp.path().join("dest/target.txt").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join("source/file.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn move_destination_change_keeps_patch_target_changed() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("source")).expect("source dir");
    fs::create_dir(temp.path().join("dest")).expect("dest dir");
    fs::write(temp.path().join("source/file.txt"), "one\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: source/file.txt\n*** Move to: dest/target.txt\n@@\n-one\n+two\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    fs::write(temp.path().join("dest/target.txt"), "occupied\n").expect("create destination");

    let incomplete = expect_incomplete(commit_planned(temp.path(), &planned));
    assert_eq!(incomplete.writes, PatchWrites::None);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unchanged);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchTargetChanged);
    assert_eq!(
        incomplete.diagnostic.path.as_deref(),
        Some("source/file.txt")
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("source/file.txt")).unwrap(),
        "one\n"
    );
}

#[cfg(unix)]
#[test]
fn move_source_path_change_reports_patch_source_changed() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("source")).expect("source dir");
    fs::create_dir(temp.path().join("dest")).expect("dest dir");
    fs::write(temp.path().join("source/file.txt"), "one\n").expect("seed");
    fs::write(temp.path().join("other.txt"), "other\n").expect("other");
    let patch = "*** Begin Patch\n*** Update File: source/file.txt\n*** Move to: dest/target.txt\n@@\n-one\n+two\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    fs::remove_file(temp.path().join("source/file.txt")).expect("remove source");
    unix_fs::symlink(
        temp.path().join("other.txt"),
        temp.path().join("source/file.txt"),
    )
    .expect("replace source with a different symlink target");

    let incomplete = expect_incomplete(commit_planned(temp.path(), &planned));
    assert_eq!(incomplete.writes, PatchWrites::None);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unchanged);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchSourceChanged);
    assert_eq!(incomplete.diagnostic.stage, PatchStage::Commit);
    assert_eq!(
        incomplete.diagnostic.path.as_deref(),
        Some("source/file.txt")
    );
    assert!(!temp.path().join("dest/target.txt").exists());
}

#[test]
fn failed_move_after_destination_write_reports_changed_effect() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("source")).expect("source dir");
    fs::create_dir(temp.path().join("dest")).expect("dest dir");
    fs::write(temp.path().join("source/file.txt"), "one\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: source/file.txt\n*** Move to: dest/target.txt\n@@\n-one\n+two\n*** Add File: tail.txt\n+tail\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(real_rename, failing_remove),
    ));
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Changed);
    assert_eq!(incomplete.changes[1].status, ApplyPatchStatus::Skipped);
    assert_eq!(incomplete.changes[1].effect, PatchEffect::Unchanged);
    assert!(incomplete.changes[1].error.is_none());
    assert_eq!(incomplete.writes, PatchWrites::Partial);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchWriteFailed);
    assert_eq!(
        fs::read_to_string(temp.path().join("dest/target.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("source/file.txt")).unwrap(),
        "one\n"
    );
    assert!(!temp.path().join("tail.txt").exists());
}

#[test]
fn failed_nested_add_does_not_report_no_effect_after_parent_creation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** Add File: created/new.txt\n+content\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");
    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(failing_rename, real_remove),
    ));
    assert!(temp.path().join("created").is_dir());
    assert_ne!(incomplete.writes, PatchWrites::None);
}

#[test]
fn add_rename_and_temp_cleanup_failure_reports_unknown_residue() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** Add File: new.txt\n+content\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(failing_rename, failing_remove),
    ));
    assert_eq!(incomplete.writes, PatchWrites::Unknown);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unknown);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchWriteFailed);
    assert!(!temp.path().join("new.txt").exists());
    let residues = fs::read_dir(temp.path())
        .expect("read tempdir")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".new.txt.desk-foreman-patch-")
        })
        .collect::<Vec<_>>();
    assert_eq!(residues.len(), 1, "exactly one temp residue is expected");
    assert_eq!(incomplete.diagnostic.message(), "patch write failed");
    let message = incomplete.diagnostic.message();
    assert!(!message.contains(temp.path().to_string_lossy().as_ref()));
}

#[test]
fn unknown_commit_effect_is_reported_as_unknown_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("blocker"), "file").expect("seed");
    let patch = "\
*** Begin Patch
*** Add File: blocker/second.txt
+two
*** End Patch";
    let incomplete = expect_incomplete(apply_patch(temp.path(), patch));
    assert_eq!(incomplete.writes, PatchWrites::Unknown);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unknown);
    assert!(!temp.path().join("blocker/second.txt").exists());
}

#[test]
fn failed_add_after_missing_parent_creation_reports_changed_effect() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** Add File: created/new.txt\n+content\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(failing_rename, real_remove),
    ));
    assert_eq!(incomplete.writes, PatchWrites::Partial);
    assert_eq!(incomplete.changes[0].operation, ApplyPatchOperation::Add);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Changed);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchWriteFailed);
    assert_eq!(
        incomplete.diagnostic.path.as_deref(),
        Some("created/new.txt")
    );
    assert_eq!(
        incomplete.diagnostic.operation,
        Some(ApplyPatchOperation::Add)
    );
    assert!(temp.path().join("created").is_dir());
    assert!(!temp.path().join("created/new.txt").exists());
}

#[test]
fn failed_move_after_missing_parent_creation_reports_changed_effect() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("source")).expect("source dir");
    fs::write(temp.path().join("source/file.txt"), "one\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: source/file.txt\n*** Move to: created/target.txt\n@@\n-one\n+two\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(failing_rename, real_remove),
    ));
    assert_eq!(incomplete.writes, PatchWrites::Partial);
    assert_eq!(incomplete.changes.len(), 1);
    assert_eq!(incomplete.changes[0].operation, ApplyPatchOperation::Move);
    assert_eq!(incomplete.changes[0].path, "source/file.txt");
    assert_eq!(
        incomplete.changes[0].destination.as_deref(),
        Some("created/target.txt")
    );
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Changed);
    assert!(incomplete.changes[0].error.is_some());
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchWriteFailed);
    assert_eq!(
        incomplete.diagnostic.path.as_deref(),
        Some("source/file.txt")
    );
    assert_eq!(
        incomplete.diagnostic.operation,
        Some(ApplyPatchOperation::Move)
    );
    assert!(
        temp.path().join("created").is_dir(),
        "created parent directory remains"
    );
    assert!(!temp.path().join("created/target.txt").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join("source/file.txt")).unwrap(),
        "one\n"
    );
}

#[test]
fn failed_move_with_existing_parent_and_cleanup_reports_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("source")).expect("source dir");
    fs::create_dir(temp.path().join("dest")).expect("dest dir");
    fs::write(temp.path().join("source/file.txt"), "one\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: source/file.txt\n*** Move to: dest/target.txt\n@@\n-one\n+two\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(failing_rename, real_remove),
    ));
    assert_eq!(incomplete.writes, PatchWrites::None);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unchanged);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchWriteFailed);
    assert!(temp.path().join("dest").is_dir());
    assert!(!temp.path().join("dest/target.txt").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join("source/file.txt")).unwrap(),
        "one\n"
    );
    assert_eq!(
        fs::read_dir(temp.path().join("dest"))
            .expect("dest dir")
            .count(),
        0,
        "successful temp cleanup leaves no residue"
    );
}

#[test]
fn failed_add_with_existing_parent_and_cleanup_reports_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** Add File: new.txt\n+content\n*** End Patch";
    let parsed = parse_patch(patch).expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");

    let incomplete = expect_incomplete(commit_planned_with(
        temp.path(),
        &planned,
        FsOps::new(failing_rename, real_remove),
    ));
    assert_eq!(incomplete.writes, PatchWrites::None);
    assert_eq!(incomplete.changes[0].operation, ApplyPatchOperation::Add);
    assert_eq!(incomplete.changes[0].status, ApplyPatchStatus::Failed);
    assert_eq!(incomplete.changes[0].effect, PatchEffect::Unchanged);
    assert_eq!(incomplete.diagnostic.code, PatchCode::PatchWriteFailed);
    assert_eq!(incomplete.diagnostic.path.as_deref(), Some("new.txt"));
    assert_eq!(
        incomplete.diagnostic.operation,
        Some(ApplyPatchOperation::Add)
    );
    assert!(!temp.path().join("new.txt").exists());
    assert_eq!(
        fs::read_dir(temp.path())
            .expect("tempdir")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".new.txt.desk-foreman-patch-")
            })
            .count(),
        0,
        "successful temp cleanup leaves no residue"
    );
}

#[test]
fn rejects_stale_content_before_commit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("a.txt");
    fs::write(&path, "one\n").expect("seed");
    let parsed =
        parse_patch("*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch")
            .expect("parse");
    let planned = plan_patch(temp.path(), &parsed).expect("plan");
    fs::write(&path, "external\n").expect("external edit");
    let outcome = commit_change(temp.path(), &planned[0], FsOps::REAL);
    assert_eq!(outcome.effect, PatchEffect::Unchanged);
    assert_eq!(outcome.error, Some(PatchCode::PatchTargetChanged));
    assert_eq!(fs::read_to_string(path).unwrap(), "external\n");
}

#[test]
fn update_rejects_non_utf8_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("binary.bin"), [0, 159]).expect("seed");
    let patch = "*** Begin Patch\n*** Update File: binary.bin\n@@\n-x\n+y\n*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), patch));
    assert_eq!(diagnostic.code, PatchCode::PatchNotUtf8Text);
    assert_eq!(diagnostic.path.as_deref(), Some("binary.bin"));
}

#[test]
fn protected_paths_are_rejected_without_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** Add File: .env\n+secret\n*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), patch));
    assert_eq!(diagnostic.code, PatchCode::PatchPathProtected);
    assert_eq!(diagnostic.path.as_deref(), Some(".env"));
    assert!(!temp.path().join(".env").exists());
}

#[test]
fn patch_context_not_found_has_fixed_diagnostic() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "hello\nworld\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-missing secret-shaped content\n+new\n*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), patch));
    assert_eq!(diagnostic.code, PatchCode::PatchContextNotFound);
    assert_eq!(diagnostic.stage, PatchStage::Plan);
    assert_eq!(diagnostic.path.as_deref(), Some("a.txt"));
    assert_eq!(diagnostic.hunk, Some(0));
    assert_eq!(diagnostic.message(), "patch context was not found");
    assert!(!diagnostic.message().contains("secret"));
}

#[test]
fn patch_context_ambiguous_has_fixed_diagnostic() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "same\nsame\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-same\n+changed\n*** End Patch";
    let diagnostic = expect_rejected(apply_patch(temp.path(), patch));
    assert_eq!(diagnostic.code, PatchCode::PatchContextAmbiguous);
    assert_eq!(diagnostic.message(), "patch context is ambiguous");
}
