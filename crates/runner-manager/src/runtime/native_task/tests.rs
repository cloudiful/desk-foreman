//! Native-task materialization behaviour tests (issue 753 P2b).
//!
//! Non-root and filesystem only: roots, ownership, byte-exactness and replay.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use super::NativeTaskStore;
use super::test_helpers::{bundle_of, current_uid, layout_of, request, root, store};
use crate::runtime::execution_environment::service::host_identity::UidSource;

#[path = "test_paths.rs"]
mod path_tests;

#[tokio::test]
async fn a_disabled_root_fails_with_a_fixed_unsupported_error() {
    let store = NativeTaskStore::with_uid_source(None, UidSource::Fixed(current_uid()));
    let request = request(&[("a.txt", "a")]);
    let error = store
        .materialize(&request, &bundle_of(&[("a.txt", "a")]))
        .await
        .expect_err("disabled store must fail");
    assert!(error.to_string().contains("not configured"), "{error}");
}

#[tokio::test]
async fn an_unavailable_uid_fails_closed() {
    let root = root();
    let store = NativeTaskStore::with_uid_source(Some(root.clone()), UidSource::Unavailable);
    let request = request(&[("a.txt", "a")]);
    assert!(
        store
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_missing_permissive_symlinked_or_foreign_root_is_refused() {
    // Missing.
    let missing = root().join("absent");
    let request = request(&[("a.txt", "a")]);
    assert!(
        store(&missing)
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );

    // Permissive.
    let permissive = root();
    fs::set_permissions(&permissive, fs::Permissions::from_mode(0o755)).expect("mode");
    assert!(
        store(&permissive)
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );

    // A symlink is never followed.
    let real = root();
    let link_parent = root();
    let link = link_parent.join("root");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    assert!(
        store(&link)
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );

    // An injected expected uid exercises the foreign-root rule without chown.
    let foreign = root();
    let foreign_store =
        NativeTaskStore::with_uid_source(Some(foreign), UidSource::Fixed(current_uid() + 1));
    assert!(
        foreign_store
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn materialization_is_byte_exact_and_owner_only() {
    let root = root();
    let store = store(&root);
    let request = request(&[("dir/a.txt", "hello"), ("b.txt", "wörld")]);
    let proof = store
        .materialize(
            &request,
            &bundle_of(&[("dir/a.txt", "hello"), ("b.txt", "wörld")]),
        )
        .await
        .expect("materialize");
    assert!(proof.prepared);
    assert_eq!(proof.task_id, request.task_id);
    assert_eq!(proof.manifest_sha256, request.manifest_sha256);

    let task_dir = layout_of(&root, &request);
    let tree = task_dir.join("tree");
    assert_eq!(
        fs::read_to_string(tree.join("dir/a.txt")).expect("read"),
        "hello"
    );
    // UTF-8 content is stored byte-exactly.
    assert_eq!(
        fs::read(tree.join("b.txt")).expect("read"),
        "wörld".as_bytes()
    );

    let meta = fs::symlink_metadata(tree.join("b.txt")).expect("stat");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let dir_meta = fs::symlink_metadata(&tree).expect("stat");
    assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);
    // The journal is owner-only and records the prepared phase.
    let journal = fs::read_to_string(task_dir.join("journal.json")).expect("journal");
    assert!(journal.contains("prepared"), "{journal}");
    assert!(
        !journal.contains("hello"),
        "no content in the journal: {journal}"
    );
}

#[tokio::test]
async fn an_empty_seed_materializes_an_empty_tree() {
    let root = root();
    let store = store(&root);
    let request = request(&[]);
    let proof = store
        .materialize(&request, &bundle_of(&[]))
        .await
        .expect("materialize empty");
    assert!(proof.prepared);
    assert_eq!(proof.file_count, 0);
    let tree = layout_of(&root, &request).join("tree");
    assert_eq!(fs::read_dir(&tree).expect("read dir").count(), 0);
}

#[tokio::test]
async fn a_repeat_prepared_call_returns_the_proof_without_rewriting() {
    let root = root();
    let store = store(&root);
    let request = request(&[("a.txt", "a")]);
    store
        .materialize(&request, &bundle_of(&[("a.txt", "a")]))
        .await
        .expect("first");
    let tree = layout_of(&root, &request).join("tree");
    let before = fs::symlink_metadata(tree.join("a.txt"))
        .expect("stat")
        .ino();
    // A second call must not rewrite the tree.
    let proof = store
        .materialize(&request, &bundle_of(&[("a.txt", "a")]))
        .await
        .expect("replay");
    assert!(proof.prepared);
    let after = fs::symlink_metadata(tree.join("a.txt"))
        .expect("stat")
        .ino();
    assert_eq!(before, after, "a replay must not rewrite the tree");
}

#[tokio::test]
async fn a_bundle_that_does_not_match_the_request_is_refused_before_any_write() {
    let root = root();
    let store = store(&root);
    let request = request(&[("a.txt", "a")]);
    // A different bundle than the announced identity.
    assert!(
        store
            .materialize(&request, &bundle_of(&[("a.txt", "different")]))
            .await
            .is_err()
    );
    let task_dir = layout_of(&root, &request);
    assert!(!task_dir.exists(), "nothing may be written on a mismatch");
}
