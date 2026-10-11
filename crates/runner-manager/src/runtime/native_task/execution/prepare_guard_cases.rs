use std::{os::unix::fs::PermissionsExt, time::Duration};

use runner_protocol::{SourceSeedSnapshotFile, validate_seed_bundle};

use super::test_support::{ENVIRONMENT_ID, Rig};

#[tokio::test]
async fn guarded_preparation_is_serialized_before_stop_and_cannot_reopen_the_generation() {
    let rig = Rig::prepared().await;
    let task_dir = rig
        .task_tree
        .parent()
        .expect("task directory")
        .to_path_buf();
    let environment_dir = task_dir.parent().expect("environment directory");
    std::fs::remove_dir_all(&task_dir).expect("remove old test checkout");
    std::fs::create_dir(&task_dir).expect("empty task directory");
    std::fs::set_permissions(&task_dir, std::fs::Permissions::from_mode(0o700))
        .expect("task directory mode");

    let uid = rig
        .backend
        .native_tasks
        .effective_uid()
        .expect("manager uid");
    let workspace = rig
        .backend
        .native_tasks
        .lock_workspace(ENVIRONMENT_ID, rig.request.task.task_id, uid)
        .await
        .expect("hold shared task mutex");
    let seed =
        validate_seed_bundle(&[SourceSeedSnapshotFile::new("README.md", "initial")]).expect("seed");
    let preparation = rig.backend.prepare_native_task_checkout_with_profile(
        &rig.profile,
        rig.prepare_request.clone(),
        seed,
    );
    tokio::pin!(preparation);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut preparation)
            .await
            .is_err()
    );

    let barrier_path = rig.stopped_barrier_path();
    let before_stop: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&barrier_path).expect("high-water barrier"))
            .expect("barrier JSON");
    assert_eq!(before_stop["high_water"], 4);
    assert!(before_stop["stopped_generation"].is_null());

    let stop = rig.stop();
    tokio::pin!(stop);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut stop)
            .await
            .is_err()
    );
    assert!(
        !rig.log().lines().any(|line| line.starts_with("stop ")),
        "stop waits behind the guarded preparation"
    );

    drop(workspace);
    let prepared = preparation
        .await
        .expect("preparation completes before stop");
    assert!(prepared.prepared);
    let stopped = stop.await.expect("stop after preparation completes");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
    assert!(rig.task_tree.is_dir());
    assert!(
        environment_dir
            .join(rig.request.task.task_id.to_string())
            .is_dir()
    );

    let late_prepare = rig
        .backend
        .prepare_native_task_checkout_with_profile(
            &rig.profile,
            rig.prepare_request.clone(),
            validate_seed_bundle(&[SourceSeedSnapshotFile::new("README.md", "initial")])
                .expect("seed"),
        )
        .await
        .expect_err("the durable stop fence rejects delayed preparation");
    assert!(
        late_prepare.to_string().contains("durable-stopped"),
        "{late_prepare}"
    );
}
