use std::{os::unix::fs::PermissionsExt, time::Duration};

use runner_protocol::{SourceSeedSnapshotFile, validate_seed_bundle};

use super::test_support::{ENVIRONMENT_ID, Rig};
use crate::runtime::execution_environment::profile::{
    command::TrustedCommand, workdir::TrustedStateWorkdir,
};

fn journal_path(rig: &Rig) -> std::path::PathBuf {
    rig.task_tree
        .parent()
        .expect("task directory")
        .join("journal.json")
}

fn seed() -> runner_protocol::SeedBundle {
    validate_seed_bundle(&[SourceSeedSnapshotFile::new("README.md", "initial")])
        .expect("prepared seed")
}

#[tokio::test]
async fn running_stop_fences_writers_and_preserves_the_edited_workspace() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    let edited_file = rig.task_tree.join("README.md");
    std::fs::write(&edited_file, "edited before stop").expect("edit task file");
    rig.set_mode("require-stop-barrier");

    let mut changed_profile = rig.profile.clone();
    changed_profile.enabled = false;
    changed_profile.command = TrustedCommand::Unset;
    changed_profile.state_workdir = TrustedStateWorkdir::Unset;
    changed_profile.volume_uid = None;
    changed_profile.volume_gid = None;
    let stopped = rig
        .stop_with_profile(&changed_profile)
        .await
        .expect("stop running task");
    assert_eq!(stopped.task, rig.request.task);
    assert!(stopped.preparation_quiescent);
    assert!(stopped.environment.confirmed);
    assert_eq!(stopped.environment.observed_state, "absent");
    assert_eq!(rig.stop_count(), 1);
    assert_eq!(
        std::fs::read_to_string(&edited_file).expect("edited task file remains"),
        "edited before stop"
    );
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path(&rig)).expect("journal remains"))
            .expect("journal JSON");
    assert_eq!(journal["phase"], "running");
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("durable stop barrier"),
    )
    .expect("barrier JSON");
    assert_eq!(barrier["stopped_generation"], 4);

    let replay = rig.stop().await.expect("idempotent stop replay");
    assert!(replay.preparation_quiescent && replay.environment.confirmed);
    assert_eq!(replay.environment.observed_state, "absent");
    assert_eq!(rig.stop_count(), 1);
    let before_late_start = rig.log();

    let reopened = rig.reopened_backend().await;
    let error = crate::runtime::start_native_task(&reopened, &rig.profile, rig.request.clone())
        .await
        .expect_err("a durable stop fences a late start after backend reopen");
    assert!(error.to_string().contains("durable-stopped"), "{error}");
    assert_eq!(rig.log(), before_late_start);
}

#[tokio::test]
async fn a_prepared_task_without_a_container_is_quiescent_and_not_cleaned_up() {
    let rig = Rig::prepared().await;
    let before = std::fs::read(journal_path(&rig)).expect("prepared journal");
    rig.set_mode("require-stop-barrier");

    let stopped = rig
        .stop()
        .await
        .expect("stop prepared task without a container");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
    assert_eq!(stopped.environment.observed_state, "absent");
    assert_eq!(rig.stop_count(), 0);
    assert_eq!(
        std::fs::read(journal_path(&rig)).expect("journal remains"),
        before
    );
    assert_eq!(
        std::fs::read_to_string(rig.task_tree.join("README.md")).expect("seed file remains"),
        "initial"
    );

    let error = rig
        .backend
        .prepare_native_task_checkout_with_profile(
            &rig.profile,
            rig.prepare_request.clone(),
            seed(),
        )
        .await
        .expect_err("the stop barrier fences a late preparation replay");
    assert!(error.to_string().contains("durable-stopped"), "{error}");
    assert_eq!(
        std::fs::read(journal_path(&rig)).expect("journal unchanged"),
        before
    );
}

#[tokio::test]
async fn an_absent_task_directory_can_be_stopped_without_creating_it() {
    let rig = Rig::prepared().await;
    let task_dir = rig.task_tree.parent().expect("task dir").to_path_buf();
    std::fs::remove_dir_all(&task_dir).expect("remove only the test-owned task directory");

    let stopped = rig.stop().await.expect("stop absent task workspace");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
    assert_eq!(stopped.environment.observed_state, "absent");
    assert!(!task_dir.exists());
}

#[tokio::test]
async fn an_interrupted_preparing_journal_is_quiescent_without_rewriting_staging() {
    let rig = Rig::prepared().await;
    let task_dir = rig.task_tree.parent().expect("task directory");
    let staging = task_dir.join(".staging");
    std::fs::create_dir(&staging).expect("staging root");
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))
        .expect("staging mode");
    let path = journal_path(&rig);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("prepared journal"))
            .expect("journal JSON");
    journal["phase"] = serde_json::json!("preparing");
    std::fs::write(&path, journal.to_string()).expect("interrupt preparation");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("journal mode");
    let interrupted = std::fs::read(&path).expect("interrupted journal");

    let stopped = rig.stop().await.expect("quiesce interrupted preparation");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
    assert_eq!(std::fs::read(&path).expect("journal remains"), interrupted);
    assert!(staging.is_dir(), "stop leaves staging for reconciliation");
}

#[tokio::test]
async fn workspace_lock_waits_for_the_writer_and_persists_stop_before_inspection() {
    let rig = Rig::prepared().await;
    let uid = rig
        .backend
        .native_tasks
        .effective_uid()
        .expect("manager uid");
    let writer = rig
        .backend
        .native_tasks
        .lock_workspace(ENVIRONMENT_ID, rig.request.task.task_id, uid)
        .await
        .expect("hold the shared task mutex");
    rig.set_mode("require-stop-barrier");
    let stop = rig.stop();
    tokio::pin!(stop);

    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut stop)
            .await
            .is_err()
    );
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("stop persisted while waiting"),
    )
    .expect("barrier JSON");
    assert_eq!(barrier["stopped_generation"], 4);
    assert!(
        rig.log().is_empty(),
        "no Docker command runs before task quiescence"
    );

    drop(writer);
    let stopped = stop.await.expect("stop after the writer releases");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
}

#[tokio::test]
async fn mount_mismatch_keeps_the_tombstone_and_never_stops_a_foreign_candidate() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    let mut info = rig.inspect_value();
    info["Mounts"][1]["Source"] = serde_json::json!("/untrusted/task/tree");
    rig.set_inspect(info);
    let before = rig.log();

    assert!(rig.stop().await.is_err());
    assert!(!rig.log()[before.len()..].contains("stop "));
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("persisted tombstone"),
    )
    .expect("barrier JSON");
    assert_eq!(barrier["stopped_generation"], 4);
}

#[tokio::test]
async fn an_overlapping_state_mount_is_not_a_valid_native_stop_candidate() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    let before = rig.log().lines().count();
    let mut info = rig.inspect_value();
    info["Mounts"][0]["Destination"] = serde_json::json!("/workspace/state");
    rig.set_inspect(info);

    assert!(rig.stop().await.is_err());
    assert!(
        !rig.log()
            .lines()
            .skip(before)
            .any(|line| line.starts_with("stop "))
    );
}

#[tokio::test]
async fn a_missing_daemon_root_for_a_present_candidate_is_not_absence() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    rig.configure_host_root(None).await;
    let before = rig.log();

    assert!(rig.stop().await.is_err());
    assert!(!rig.log()[before.len()..].contains("stop "));
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("persisted tombstone"),
    )
    .expect("barrier JSON");
    assert_eq!(barrier["stopped_generation"], 4);
}

#[tokio::test]
async fn a_stuck_candidate_never_returns_a_positive_stop_proof() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    rig.set_mode("stop-stuck");
    let before = std::fs::read(journal_path(&rig)).expect("running journal");

    assert!(rig.stop().await.is_err());
    assert_eq!(rig.stop_count(), 1);
    assert_eq!(
        std::fs::read(journal_path(&rig)).expect("journal unchanged"),
        before
    );
    assert!(rig.stopped_barrier_path().exists());
}

#[path = "stop_cases/race_cases.rs"]
mod race_cases;

#[tokio::test]
async fn stop_does_not_read_or_rewrite_task_created_descendants() {
    use std::os::unix::fs::symlink;

    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    let link = rig.task_tree.join("task-created-link");
    symlink("/etc/passwd", &link).expect("task-created symlink");
    let journal = std::fs::read(journal_path(&rig)).expect("running journal");

    let stopped = rig.stop().await.expect("stop task with edited descendants");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("link remains")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read(journal_path(&rig)).expect("journal unchanged"),
        journal
    );
}
