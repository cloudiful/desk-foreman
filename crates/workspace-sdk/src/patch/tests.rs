use std::fs;

use crate::{ApplyPatchStatus, parse_patch};

use super::{
    apply::{apply_patch, commit_planned},
    commit::commit_change,
    plan::plan_patch,
};

#[test]
fn parses_and_applies_add_update_delete_and_move() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    fs::write(root.join("hello.txt"), "hello\nworld\n").expect("seed");

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
*** End Patch";

    let summary = apply_patch(root, patch).expect("patch should apply");
    assert!(!summary.partial);
    assert_eq!(summary.changes.len(), 2);
    assert!(summary.summary.contains("M hello.txt -> moved.txt"));
    assert_eq!(
        fs::read_to_string(root.join("moved.txt")).expect("moved file"),
        "hello\ndesk foreman\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("new.txt")).expect("new file"),
        "fresh\n"
    );
    assert!(!root.join("hello.txt").exists());
}

#[test]
fn rejects_empty_and_bad_grammar() {
    assert!(parse_patch("*** Begin Patch\n*** End Patch\n").is_err());
    assert!(parse_patch("bad").is_err());
    assert!(parse_patch("*** Begin Patch\n*** Add File: x\n*** End Patch").is_err());
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
    assert!(apply_patch(temp.path(), patch).is_err());
    assert!(!temp.path().join("a.txt").exists());

    let absolute = "*** Begin Patch\n*** Add File: /tmp/nope\n+x\n*** End Patch";
    assert!(apply_patch(temp.path(), absolute).is_err());
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

    let summary = apply_patch(temp.path(), patch).expect("fallback should apply");
    assert_eq!(summary.changes[0].status, ApplyPatchStatus::Applied);
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
    assert!(apply_patch(temp.path(), patch).is_err());
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
    apply_patch(temp.path(), patch).expect("eof patch");
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

    apply_patch(temp.path(), patch).expect("blank line patch");
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
    apply_patch(temp.path(), patch).expect("line ending patch");
    assert_eq!(
        fs::read(temp.path().join("a.txt")).unwrap(),
        b"\xef\xbb\xbfone\r\nthree\r\n"
    );
}

#[test]
fn delete_accepts_non_utf8_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("binary.bin"), [0, 159, 146, 150]).expect("seed");
    let patch = "*** Begin Patch\n*** Delete File: binary.bin\n*** End Patch";
    let summary = apply_patch(temp.path(), patch).expect("delete binary");
    assert!(!temp.path().join("binary.bin").exists());
    assert_eq!(summary.changes[0].deleted_lines, 1);
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

    let summary = commit_planned(temp.path(), &planned);
    let summary = summary.expect("partial result");
    assert!(summary.partial);
    assert_eq!(summary.changes[0].status, ApplyPatchStatus::Applied);
    assert_eq!(summary.changes[1].status, ApplyPatchStatus::Failed);
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
fn rejects_commit_when_parent_is_not_a_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("blocker"), "file").expect("seed");
    let patch = "\
*** Begin Patch
*** Add File: blocker/second.txt
+two
*** End Patch";
    assert!(apply_patch(temp.path(), patch).is_err());
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
    let error = commit_change(temp.path(), &planned[0]).expect_err("stale content");
    assert!(error.contains("changed while applying patch"));
    assert_eq!(fs::read_to_string(path).unwrap(), "external\n");
}

#[test]
fn update_rejects_non_utf8_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("binary.bin"), [0, 159]).expect("seed");
    let patch = "*** Begin Patch\n*** Update File: binary.bin\n@@\n-x\n+y\n*** End Patch";
    assert!(apply_patch(temp.path(), patch).is_err());
}

#[test]
fn patch_context_not_found_has_stable_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "hello\nworld\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-missing\n+new\n*** End Patch";
    let error = apply_patch(temp.path(), patch).expect_err("context mismatch should fail");
    assert_eq!(error.code(), "patch_context_not_found");
    let message = error.to_string();
    assert!(
        message.contains("unable to find patch context")
            || message.contains("failed to apply hunks"),
        "message should indicate context failure, got {message}"
    );
    match error {
        crate::WorkspaceSdkError::PatchContextNotFound(_) => {}
        other => panic!("expected PatchContextNotFound, got {other:?}"),
    }
}

#[test]
fn patch_context_ambiguous_has_stable_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("a.txt"), "same\nsame\n").expect("seed");
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-same\n+changed\n*** End Patch";
    let error = apply_patch(temp.path(), patch).expect_err("ambiguous should fail");
    assert_eq!(error.code(), "patch_context_not_found");
    assert!(matches!(
        error,
        crate::WorkspaceSdkError::PatchContextNotFound(_)
    ));
}

#[test]
fn generic_invalid_input_remains_generic_code() {
    let error = parse_patch("*** Begin Patch\n*** End Patch\n").expect_err("empty patch");
    assert_eq!(error.code(), "invalid_input");
    assert!(matches!(error, crate::WorkspaceSdkError::InvalidInput(_)));
}
