use std::{os::unix::fs::PermissionsExt, time::Duration};

use super::super::test_support::{ENVIRONMENT_ID, Rig};
use super::journal_path;

#[tokio::test]
async fn stop_waits_for_an_active_start_before_recording_quiescence() {
    let rig = Rig::prepared().await;
    rig.set_mode("slow-run");
    let backend = rig.backend.clone();
    let profile = rig.profile.clone();
    let request = rig.request.clone();
    let start = tokio::spawn(async move {
        crate::runtime::start_native_task(&backend, &profile, request).await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !rig.log().lines().any(|line| line.starts_with("run ")) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("start reaches the container-create command");

    let stop = rig.stop();
    tokio::pin!(stop);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut stop)
            .await
            .is_err()
    );
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("start high-water barrier"),
    )
    .expect("barrier JSON");
    assert!(barrier["stopped_generation"].is_null());
    assert_eq!(rig.stop_count(), 0);

    start
        .await
        .expect("start task joins")
        .expect("active start completes");
    let stopped = stop
        .await
        .expect("stop after active start releases the locks");
    assert!(stopped.preparation_quiescent && stopped.environment.confirmed);
    assert_eq!(rig.stop_count(), 1);
    let log = rig.log();
    let run = log
        .lines()
        .position(|line| line.starts_with("run "))
        .expect("run command");
    let stop = log
        .lines()
        .position(|line| line.starts_with("stop "))
        .expect("stop command");
    assert!(
        run < stop,
        "container start completes before its stop: {log}"
    );
    let before_late_start = rig.log();
    let error = rig
        .start()
        .await
        .expect_err("persistent tombstone fences a delayed start");
    assert!(error.to_string().contains("durable-stopped"), "{error}");
    assert_eq!(rig.log(), before_late_start);
}

#[tokio::test]
async fn missing_local_root_for_a_present_candidate_is_not_treated_as_absence() {
    let rig = Rig::prepared_without_local_root().await;
    rig.mark_container_exists();
    let error = rig
        .stop()
        .await
        .expect_err("local task identity root is required");
    assert!(error.to_string().contains("not configured"), "{error}");
    assert!(
        rig.log().is_empty(),
        "stop refuses before Docker inspection"
    );
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("durable tombstone"),
    )
    .expect("barrier JSON");
    assert_eq!(barrier["stopped_generation"], 4);
}

#[tokio::test]
async fn deadline_expiry_while_waiting_for_the_workspace_never_confirms_quiescence() {
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
        .expect("hold task workspace lock");
    let mut profile = rig.profile.clone();
    profile.operation_deadline = Duration::from_millis(100);

    let error = rig
        .stop_with_profile(&profile)
        .await
        .expect_err("deadline expires before preparation quiescence");
    assert!(
        error
            .to_string()
            .contains("timed out waiting for native task preparation")
    );
    let barrier: serde_json::Value = serde_json::from_slice(
        &std::fs::read(rig.stopped_barrier_path()).expect("durable stop intent"),
    )
    .expect("barrier JSON");
    assert_eq!(barrier["stopped_generation"], 4);
    assert!(
        rig.log().is_empty(),
        "no Docker command runs before the workspace lock"
    );
    drop(writer);
}

#[tokio::test]
async fn a_container_candidate_without_its_journal_is_never_stopped_as_absent() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("start native task");
    let journal = rig
        .task_tree
        .parent()
        .expect("task directory")
        .join("journal.json");
    std::fs::remove_file(journal).expect("remove test-owned journal");
    let before = rig.log();

    assert!(rig.stop().await.is_err());
    assert!(!rig.log()[before.len()..].contains("stop "));
    assert!(rig.stopped_barrier_path().exists());
}

#[tokio::test]
async fn a_foreign_journal_or_unsafe_task_root_never_produces_quiescence() {
    let rig = Rig::prepared().await;
    let path = journal_path(&rig);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("prepared journal"))
            .expect("journal JSON");
    journal["manifest_sha256"] = serde_json::json!("f".repeat(64));
    std::fs::write(&path, journal.to_string()).expect("foreign journal fixture");
    assert!(rig.stop().await.is_err());
    assert!(
        rig.log().is_empty(),
        "foreign journal is rejected before Docker"
    );

    let rig = Rig::prepared().await;
    std::fs::set_permissions(&rig.task_tree, std::fs::Permissions::from_mode(0o755))
        .expect("broaden task root permissions");
    assert!(rig.stop().await.is_err());
    assert!(
        rig.log().is_empty(),
        "unsafe task root is rejected before Docker"
    );
}
