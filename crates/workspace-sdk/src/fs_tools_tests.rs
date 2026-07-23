use std::fs;

use crate::{
    FileFingerprint, ListDirectoryPageParams, PathKind, ReadFilePageParams, StatPathParams,
    WalkWorkspacePageParams,
};

use super::WorkspaceFileTools;

#[test]
fn read_file_page_is_bounded_and_utf8_safe() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("notes.txt"), "alpha\nbeta\ngamma\n").expect("write");
    fs::write(temp.path().join("unicode.txt"), "你好世界\n").expect("write");
    let tools = WorkspaceFileTools::new(temp.path()).expect("tools");

    let page = tools
        .read_file_page(&ReadFilePageParams {
            path: "notes.txt".to_string(),
            start_line: 2,
            max_lines: 1,
            max_bytes: 5,
        })
        .expect("page");
    assert_eq!(page.content, "beta\n");
    assert_eq!(page.total_lines, 3);
    assert!(page.truncated);

    let unicode = tools
        .read_file_page(&ReadFilePageParams {
            path: "unicode.txt".to_string(),
            start_line: 1,
            max_lines: 1,
            max_bytes: 4,
        })
        .expect("unicode page");
    assert_eq!(unicode.content, "你");
    assert!(unicode.truncated);
}

#[test]
fn list_directory_page_returns_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("b_dir")).expect("mkdir");
    fs::write(temp.path().join("a.txt"), "a").expect("write");
    let tools = WorkspaceFileTools::new(temp.path()).expect("tools");

    let page = tools
        .list_directory_page(&ListDirectoryPageParams {
            path: ".".to_string(),
            offset: 1,
            limit: 1,
        })
        .expect("directory page");
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].path, "b_dir");
    assert_eq!(page.total_entries, 2);
    assert!(!page.truncated);
}

#[test]
fn stat_and_fingerprint_report_file_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write");
    let tools = WorkspaceFileTools::new(temp.path()).expect("tools");

    let dir = tools
        .stat_path(&StatPathParams {
            path: "src".to_string(),
        })
        .expect("stat dir");
    assert_eq!(dir.kind, PathKind::Dir);

    let first = tools
        .fingerprint_path(&StatPathParams {
            path: "src/main.rs".to_string(),
        })
        .expect("fingerprint");
    assert!(matches!(
        first,
        FileFingerprint {
            kind: PathKind::File,
            sha256: Some(_),
            ..
        }
    ));
    fs::write(
        temp.path().join("src/main.rs"),
        "fn main() { println!(\"changed\"); }\n",
    )
    .expect("rewrite");
    let second = tools
        .fingerprint_path(&StatPathParams {
            path: "src/main.rs".to_string(),
        })
        .expect("fingerprint after change");
    assert_ne!(first.sha256, second.sha256);
}

#[test]
fn walk_workspace_page_returns_bounded_recursive_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src/nested")).expect("mkdirs");
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write");
    fs::write(temp.path().join("src/lib.rs"), "pub fn run() {}\n").expect("write");
    fs::write(
        temp.path().join("src/nested/mod.rs"),
        "pub fn nested() {}\n",
    )
    .expect("write");
    let tools = WorkspaceFileTools::new(temp.path()).expect("tools");

    let page = tools
        .walk_workspace_page(&WalkWorkspacePageParams {
            path: ".".to_string(),
            max_entries: 2,
            max_depth: None,
        })
        .expect("walk page");
    assert_eq!(page.entries.len(), 2);
    assert!(page.truncated);
}

#[cfg(feature = "approval")]
#[tokio::test]
async fn reviewed_patch_does_not_commit_when_reviewer_denies() {
    use desk_foreman_approval::{
        ApprovalFuture, ApprovalReviewer, ReviewDecision, ReviewDecisionKind, ReviewRequest,
        ReviewRisk,
    };

    struct DenyReviewer;

    impl ApprovalReviewer for DenyReviewer {
        fn provider_name(&self) -> &'static str {
            "test"
        }

        fn review<'a>(&'a self, _request: &'a ReviewRequest) -> ApprovalFuture<'a> {
            Box::pin(async {
                Ok(ReviewDecision {
                    decision: ReviewDecisionKind::Deny,
                    risk: ReviewRisk::High,
                    reason_code: "test_denied".to_string(),
                    rationale: "test denial".to_string(),
                    safer_alternative: None,
                })
            })
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("notes.txt"), "old\n").expect("write");
    let tools = WorkspaceFileTools::new(temp.path()).expect("tools");
    let error = tools
        .with_reviewer(DenyReviewer)
        .apply_patch_text(
            "*** Begin Patch\n*** Update File: notes.txt\n@@\n-old\n+new\n*** End Patch",
        )
        .await
        .expect_err("denied patch must not commit");
    assert!(error.to_string().contains("approval denied"));
    assert_eq!(
        fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
        "old\n"
    );
}
