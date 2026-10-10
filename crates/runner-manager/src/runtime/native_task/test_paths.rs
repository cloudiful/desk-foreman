//! Root and tree path traversal regressions for native-task materialization.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::super::test_helpers::{bundle_of, current_uid, layout_of, request, root, store};

#[tokio::test]
async fn a_root_with_a_symlinked_ancestor_or_relative_path_is_refused() {
    let target_parent = root();
    let target_root = target_parent.join("task-root");
    fs::create_dir(&target_root).expect("target root");
    fs::set_permissions(&target_root, fs::Permissions::from_mode(0o700)).expect("root mode");

    let link_parent = root();
    let link = link_parent.join("parent");
    std::os::unix::fs::symlink(&target_parent, &link).expect("ancestor symlink");
    let request = request(&[("a.txt", "a")]);
    assert!(
        store(&link.join("task-root"))
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err(),
        "a symlink in the configured-root chain must be refused"
    );
    assert!(
        super::super::paths::validate_root(std::path::Path::new("relative-root"), current_uid())
            .is_err()
    );

    let file_parent = root();
    let not_directory = file_parent.join("not-directory");
    fs::write(&not_directory, b"file").expect("ancestor file");
    assert!(
        store(&not_directory.join("native-root"))
            .materialize(&request, &bundle_of(&[("a.txt", "a")]))
            .await
            .is_err(),
        "a non-directory ancestor must be refused"
    );
}

#[test]
fn private_directories_require_the_expected_uid_and_exact_mode() {
    let directory = root();
    assert!(super::super::paths::validate_private_dir(&directory, current_uid() + 1).is_err());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o710)).expect("mode");
    assert!(super::super::paths::validate_private_dir(&directory, current_uid()).is_err());
}

#[tokio::test]
async fn a_nested_symlink_with_matching_bytes_is_refused_on_replay() {
    let root_dir = root();
    let store = store(&root_dir);
    let request = request(&[("nested/file.txt", "same bytes")]);
    let bundle = bundle_of(&[("nested/file.txt", "same bytes")]);
    store
        .materialize(&request, &bundle)
        .await
        .expect("initial materialization");

    let task_dir = layout_of(&root_dir, &request);
    let tree = task_dir.join("tree");
    let outside = root();
    fs::write(outside.join("file.txt"), b"same bytes").expect("outside content");
    fs::set_permissions(outside.join("file.txt"), fs::Permissions::from_mode(0o600))
        .expect("outside file mode");
    fs::remove_dir_all(tree.join("nested")).expect("remove nested directory");
    std::os::unix::fs::symlink(&outside, tree.join("nested")).expect("nested symlink");

    assert!(
        store.materialize(&request, &bundle).await.is_err(),
        "replay must validate each path component, not only the leaf bytes"
    );
    assert_eq!(
        fs::read(outside.join("file.txt")).expect("outside still intact"),
        b"same bytes"
    );
}

#[tokio::test]
async fn a_published_hardlinked_file_is_refused_on_replay() {
    let root_dir = root();
    let store = store(&root_dir);
    let request = request(&[("file.txt", "same bytes")]);
    let bundle = bundle_of(&[("file.txt", "same bytes")]);
    store
        .materialize(&request, &bundle)
        .await
        .expect("initial materialization");
    let tree = layout_of(&root_dir, &request).join("tree");
    let outside = root().join("file.txt");
    fs::hard_link(tree.join("file.txt"), &outside).expect("hard link");

    assert!(
        store.materialize(&request, &bundle).await.is_err(),
        "a published file with more than one link must not be trusted"
    );
    assert_eq!(
        fs::read(outside).expect("linked file remains"),
        b"same bytes"
    );
}

#[tokio::test]
async fn invalid_seed_paths_are_refused_before_any_task_directory_is_created() {
    let root = root();
    let mut request = request(&[]);
    let files = vec![runner_protocol::SourceSeedSnapshotFile::new(
        "../escaped.txt",
        "x",
    )];
    let manifest_sha256 = runner_protocol::manifest_sha256(&files);
    request.manifest_sha256 = manifest_sha256.clone();
    request.file_count = 1;
    request.total_bytes = 1;
    let seed = runner_protocol::SeedBundle {
        files,
        manifest_sha256,
        file_count: 1,
        total_bytes: 1,
    };

    assert!(store(&root).materialize(&request, &seed).await.is_err());
    assert!(!layout_of(&root, &request).exists());
}
