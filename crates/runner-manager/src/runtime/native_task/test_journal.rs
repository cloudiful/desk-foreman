//! Native-task journal and crash-recovery tests (issue 753 P2b).
//!
//! Split from `tests.rs` so no file exceeds the size budget: interrupted,
//! corrupt, foreign and concurrent journals.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::test_helpers::{bundle_of, current_uid, layout_of, request, root, store};

#[tokio::test]
async fn an_interrupted_preparing_journal_is_retained_and_refuses() {
    let root = root();
    let store = store(&root);
    let request = request(&[("a.txt", "a")]);
    let task_dir = layout_of(&root, &request);
    fs::create_dir_all(&task_dir).expect("task dir");
    fs::set_permissions(&task_dir, fs::Permissions::from_mode(0o700)).expect("mode");
    fs::set_permissions(
        task_dir.parent().expect("environment dir"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("mode");
    let journal = task_dir.join("journal.json");
    fs::write(
        &journal,
        r#"{"version":1,"phase":"preparing","environment_id":"x","generation":4,"task_id":"00000000-0000-0000-0000-000000000001","workspace_binding_id":1,"manifest_sha256":"x","file_count":0,"total_bytes":0}"#,
    )
    .expect("write journal");
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).expect("journal mode");

    let error = store
        .materialize(&request, &bundle_of(&[("a.txt", "a")]))
        .await
        .expect_err("interrupted journal must refuse");
    assert!(error.to_string().contains("interrupted"), "{error}");
    // The interrupted journal and its task dir are never wiped.
    assert!(journal.exists());
    assert!(task_dir.exists());
}

#[tokio::test]
async fn a_foreign_or_corrupt_journal_is_refused() {
    let root = root();
    let store = store(&root);
    let request = request(&[("a.txt", "a")]);
    let task_dir = layout_of(&root, &request);
    fs::create_dir_all(&task_dir).expect("task dir");
    // Corrupt.
    let journal = task_dir.join("journal.json");
    fs::write(&journal, b"not json").expect("write");
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).expect("journal mode");
    assert!(
        store
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn journal_symlinks_modes_hardlinks_and_foreign_expected_uids_are_refused() {
    let root = root();
    let store = store(&root);
    let first = request(&[("a.txt", "a")]);
    let bundle = bundle_of(&[("a.txt", "a")]);
    store
        .materialize(&first, &bundle)
        .await
        .expect("initial materialization");
    let first_journal = layout_of(&root, &first).join("journal.json");

    // The same owner but a different expected uid exercises the metadata guard
    // without privileged chown.
    assert!(super::journal::read(&first_journal, current_uid() + 1).is_err());

    let mut second = first.clone();
    second.task_id = uuid::Uuid::new_v4();
    let second_dir = layout_of(&root, &second);
    fs::create_dir(&second_dir).expect("second task dir");
    fs::set_permissions(&second_dir, fs::Permissions::from_mode(0o700)).expect("task mode");
    std::os::unix::fs::symlink(&first_journal, second_dir.join("journal.json"))
        .expect("journal symlink");
    assert!(
        store.materialize(&second, &bundle).await.is_err(),
        "a journal symlink is refused before opening its target"
    );

    // A hard link or mode change on a valid journal is also never replayed.
    let hardlink = root.join("journal-link");
    fs::hard_link(&first_journal, &hardlink).expect("journal hardlink");
    assert!(store.materialize(&first, &bundle).await.is_err());
    fs::remove_file(&hardlink).expect("remove hardlink");
    fs::set_permissions(&first_journal, fs::Permissions::from_mode(0o640)).expect("journal mode");
    assert!(store.materialize(&first, &bundle).await.is_err());
}

#[tokio::test]
async fn a_prepared_journal_with_a_different_identity_is_refused() {
    let root = root();
    let store = store(&root);
    let first = request(&[("a.txt", "a")]);
    store
        .materialize(&first, &bundle_of(&[("a.txt", "a")]))
        .await
        .expect("first");
    // A second request that reuses the same task directory but a different
    // identity is refused rather than clobbering the first checkout.
    let mut second = first.clone();
    second.workspace_binding_id = 99;
    assert!(
        store
            .materialize(&second, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_materializations_publish_exactly_once() {
    let root = root();
    let store = std::sync::Arc::new(store(&root));
    let request = request(&[("a.txt", "a"), ("b/c.txt", "c")]);
    let bundle = bundle_of(&[("a.txt", "a"), ("b/c.txt", "c")]);

    let first = {
        let store = std::sync::Arc::clone(&store);
        let request = request.clone();
        let bundle = bundle.clone();
        tokio::spawn(async move { store.materialize(&request, &bundle).await })
    };
    let second = {
        let store = std::sync::Arc::clone(&store);
        let request = request.clone();
        let bundle = bundle.clone();
        tokio::spawn(async move { store.materialize(&request, &bundle).await })
    };
    let (first, second) = tokio::join!(first, second);
    // Both observe the same published checkout as a proof; neither errors.
    assert!(first.expect("join").expect("first").prepared);
    assert!(second.expect("join").expect("second").prepared);
    let tree = layout_of(&root, &request).join("tree");
    assert!(tree.join("a.txt").exists());
}
