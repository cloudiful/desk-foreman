use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::net::UnixListener;

use super::reader::selected_text;

fn test_root() -> tempfile::TempDir {
    let cache = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        .join(".cache/opencode/df-native-task-export/reader-tests");
    fs::create_dir_all(&cache).unwrap();
    tempfile::tempdir_in(cache).unwrap()
}

#[test]
fn native_task_export_existing_barrier_never_creates_or_repairs() {
    use crate::runtime::execution_environment::tombstone::TombstoneStore;
    let root = test_root();
    let environment = uuid::Uuid::new_v4();
    let expected = root.path().join(environment.simple().to_string());
    assert!(TombstoneStore::open_existing(root.path(), &environment.to_string()).is_err());
    assert!(!expected.exists());
    fs::create_dir(&expected).unwrap();
    let store = TombstoneStore::open_existing(root.path(), &environment.to_string()).unwrap();
    assert_eq!(store.load().unwrap().stopped_generation, None);
    assert!(!store.barrier_path().exists());
    fs::remove_dir(&expected).unwrap();
    symlink(root.path(), &expected).unwrap();
    assert!(TombstoneStore::open_existing(root.path(), &environment.to_string()).is_err());
}

#[test]
fn native_task_export_protected_tree_replacement_is_rejected() {
    let root = test_root();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let uid = fs::metadata(root.path()).unwrap().uid();
    let environment = root.path().join("environment");
    let task = environment.join("task");
    let tree = task.join("tree");
    for path in [&environment, &task, &tree] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let roots = super::roots::ProtectedRoots::capture(&tree, uid).unwrap();
    roots.verify(uid).unwrap();
    fs::rename(&tree, task.join("retained")).unwrap();
    fs::create_dir(&tree).unwrap();
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(roots.verify(uid).is_err());
}

#[test]
fn native_task_export_selected_text_and_missing() {
    let root = test_root();
    let uid = fs::metadata(root.path()).unwrap().uid();
    fs::create_dir(root.path().join("added")).unwrap();
    fs::set_permissions(root.path().join("added"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.path().join("added/text"), "edited\n").unwrap();
    fs::set_permissions(
        root.path().join("added/text"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    fs::write(root.path().join("empty"), "").unwrap();
    fs::set_permissions(root.path().join("empty"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(root.path().join("unselected"), [0, 255]).unwrap();
    assert_eq!(
        selected_text(root.path(), "added/text", uid).unwrap(),
        Some("edited\n".into())
    );
    assert_eq!(
        selected_text(root.path(), "empty", uid).unwrap(),
        Some(String::new())
    );
    assert_eq!(selected_text(root.path(), "deleted", uid).unwrap(), None);
    assert_eq!(
        selected_text(root.path(), "absent/child", uid).unwrap(),
        None
    );
    assert_eq!(fs::read(root.path().join("unselected")).unwrap(), [0, 255]);
}

#[test]
fn native_task_export_rejects_unsafe_entries_without_opening_special_files() {
    let root = test_root();
    let uid = fs::metadata(root.path()).unwrap().uid();
    fs::write(root.path().join("text"), "safe").unwrap();
    fs::hard_link(root.path().join("text"), root.path().join("hard")).unwrap();
    symlink("text", root.path().join("link")).unwrap();
    symlink("missing", root.path().join("parent")).unwrap();
    let _socket = UnixListener::bind(root.path().join("socket")).unwrap();
    fs::create_dir(root.path().join("directory")).unwrap();
    for path in [
        "text",
        "hard",
        "link",
        "parent/child",
        "socket",
        "directory",
        "../outside",
        ".git/config",
    ] {
        assert!(selected_text(root.path(), path, uid).is_err(), "{path}");
    }
}

#[test]
fn native_task_export_rejects_content_and_permission_bounds() {
    let root = test_root();
    let uid = fs::metadata(root.path()).unwrap().uid();
    for (name, bytes) in [
        ("nul", vec![0]),
        ("binary", vec![255]),
        (
            "large",
            vec![b'a'; runner_protocol::MAX_SEED_CONTENT_BYTES + 1],
        ),
    ] {
        fs::write(root.path().join(name), bytes).unwrap();
        fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(selected_text(root.path(), name, uid).is_err());
    }
    fs::write(root.path().join("writable"), "text").unwrap();
    fs::set_permissions(
        root.path().join("writable"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert!(selected_text(root.path(), "writable", uid).is_err());
    assert!(selected_text(root.path(), "writable", uid + 1).is_err());
    fs::create_dir(root.path().join("writable_dir")).unwrap();
    fs::set_permissions(
        root.path().join("writable_dir"),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(selected_text(root.path(), "writable_dir/missing", uid).is_err());
}
