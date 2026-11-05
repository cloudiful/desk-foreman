use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::install_target::{InstallTarget, InstallTargetError};
use tokio::sync::watch;
use uuid::Uuid;

fn mode(path: &Path, bits: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).expect("mode");
}

fn workspace() -> (tempfile::TempDir, i64, Uuid, PathBuf) {
    let temp = tempfile::tempdir().expect("temp");
    mode(temp.path(), 0o700);
    let app = 42;
    let project = Uuid::new_v4();
    let resources = temp
        .path()
        .join("apps")
        .join(app.to_string())
        .join("resources");
    std::fs::create_dir_all(&resources).expect("resources");
    mode(&temp.path().join("apps"), 0o700);
    mode(&temp.path().join("apps").join(app.to_string()), 0o700);
    mode(&resources, 0o700);
    let target = resources.join(desk_foreman::workspace::resource_workspace_hash(
        "code_project",
        &project.to_string(),
    ));
    (temp, app, project, target)
}

fn uid_gid() -> (u32, u32) {
    (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    )
}

fn open_target(
    root: &Path,
    app: i64,
    project: Uuid,
    target: &Path,
) -> Result<InstallTarget, InstallTargetError> {
    let (uid, gid) = uid_gid();
    InstallTarget::open(
        root,
        app,
        project,
        target.to_str().expect("target"),
        uid,
        gid,
    )
}

#[test]
fn publishes_only_the_exact_absent_target_and_cleans_temp() {
    let (temp, app, project, target) = workspace();
    let mut install = open_target(temp.path(), app, project, &target).expect("open");
    let temp_name = install.temp_fd().as_raw_fd();
    assert!(temp_name >= 0);
    let (tx, rx) = watch::channel(false);
    install
        .publish(&rx, tokio::time::Instant::now() + Duration::from_secs(5))
        .expect("publish");
    drop(tx);
    assert!(target.is_dir());
    assert!(install.cleanup().is_ok());
    assert!(target.is_dir());
}

#[test]
fn wrong_root_nonempty_target_and_symlink_parent_fail_closed() {
    let (temp, app, project, target) = workspace();
    assert!(matches!(
        open_target(temp.path(), app, project, &temp.path().join("other")),
        Err(InstallTargetError::InvalidPath)
    ));
    std::fs::create_dir(&target).expect("target");
    mode(&target, 0o700);
    std::fs::write(target.join("user-file"), b"x").expect("file");
    assert!(matches!(
        open_target(temp.path(), app, project, &target),
        Err(InstallTargetError::TargetNotEmpty)
    ));

    let (temp, app, project, target) = workspace();
    let app_dir = temp.path().join("apps").join(app.to_string());
    let resources = app_dir.join("resources");
    std::fs::remove_dir(&resources).expect("remove resources");
    std::os::unix::fs::symlink(temp.path().join("elsewhere"), &resources).expect("symlink");
    assert!(matches!(
        open_target(temp.path(), app, project, &target),
        Err(InstallTargetError::UnsafeParent)
    ));
}

#[test]
fn publication_race_never_overwrites_a_nonempty_target() {
    let (temp, app, project, target) = workspace();
    let mut install = open_target(temp.path(), app, project, &target).expect("open");
    std::fs::create_dir(&target).expect("target");
    mode(&target, 0o700);
    std::fs::write(target.join("racy"), b"x").expect("file");
    let (_tx, rx) = watch::channel(false);
    assert_eq!(
        install.publish(&rx, tokio::time::Instant::now() + Duration::from_secs(5)),
        Err(InstallTargetError::TargetNotEmpty)
    );
    assert_eq!(std::fs::read(target.join("racy")).expect("racy"), b"x");
}

#[test]
fn cancelled_publish_leaves_no_target() {
    let (temp, app, project, target) = workspace();
    let mut install = open_target(temp.path(), app, project, &target).expect("open");
    let (tx, rx) = watch::channel(false);
    tx.send(true).expect("cancel");
    assert_eq!(
        install.publish(&rx, tokio::time::Instant::now() + Duration::from_secs(5)),
        Err(InstallTargetError::Cancelled)
    );
    assert!(!target.exists());
    assert!(install.cleanup().is_ok());
}

use std::os::fd::AsRawFd;
