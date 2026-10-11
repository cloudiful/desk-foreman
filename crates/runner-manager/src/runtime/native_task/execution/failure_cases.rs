use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    time::Duration,
};

use super::test_support::Rig;

#[tokio::test]
async fn missing_or_ambiguous_host_roots_fail_before_docker() {
    let rig = Rig::prepared().await;
    rig.configure_host_root(None).await;
    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());

    rig.configure_host_root(Some("/daemon/tasks,ambiguous".into()))
        .await;
    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn a_missing_local_root_refuses_start_before_docker() {
    let rig = Rig::prepared_without_local_root().await;

    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn a_symlinked_local_root_refuses_start_before_docker() {
    let rig = Rig::prepared_with_local_root_alias().await;

    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn a_profile_uid_that_differs_from_the_manager_refuses_before_docker() {
    let mut rig = Rig::prepared().await;
    let manager_uid = std::fs::metadata(&rig.task_root)
        .expect("task root metadata")
        .uid();
    rig.set_profile_uid(manager_uid + 1);
    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn a_stopped_generation_blocks_start_before_any_docker_command() {
    let rig = Rig::prepared().await;
    let barrier = rig.stopped_barrier_path();
    std::fs::create_dir_all(barrier.parent().expect("barrier parent")).expect("barrier directory");
    std::fs::write(
        &barrier,
        r#"{"version":1,"high_water":4,"stopped_generation":4}"#,
    )
    .expect("write stopped barrier");
    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn a_prestart_tree_change_does_not_reach_docker() {
    let rig = Rig::prepared().await;
    std::fs::write(rig.task_tree.join("README.md"), "changed before start")
        .expect("change prepared tree");
    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn an_unexpected_prestart_directory_is_refused_before_docker() {
    let rig = Rig::prepared().await;
    let unexpected = rig.task_tree.join("unlisted-empty-directory");
    std::fs::create_dir(&unexpected).expect("extra directory");
    std::fs::set_permissions(&unexpected, std::fs::Permissions::from_mode(0o700))
        .expect("private extra directory");

    assert!(rig.start().await.is_err());
    assert!(rig.log().is_empty());
}

#[tokio::test]
async fn a_failed_start_is_retained_and_never_automatically_retried() {
    let rig = Rig::prepared().await;
    rig.set_mode("fail-run");
    assert!(rig.start().await.is_err());
    assert!(rig.log().contains("run "));
    let log_after_failure = rig.log();

    rig.set_mode("ok");
    assert!(rig.start().await.is_err());
    assert_eq!(rig.log(), log_after_failure);
    let journal_path = rig
        .task_tree
        .parent()
        .expect("task directory")
        .join("journal.json");
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path).expect("journal"))
            .expect("journal JSON");
    assert_eq!(journal["phase"], "starting");
}

#[tokio::test]
async fn a_docker_timeout_after_container_creation_retains_starting_without_retry() {
    let mut rig = Rig::prepared().await;
    rig.set_operation_deadline(Duration::from_millis(300));
    rig.set_mode("timeout-after-create");

    assert!(rig.start().await.is_err());
    assert_eq!(rig.run_count(), 1);
    assert!(rig.log().contains("run "));
    let journal_path = rig
        .task_tree
        .parent()
        .expect("task directory")
        .join("journal.json");
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path).expect("journal"))
            .expect("journal JSON");
    assert_eq!(journal["phase"], "starting");

    rig.set_mode("ok");
    assert!(rig.start().await.is_err());
    assert_eq!(rig.run_count(), 1, "starting candidate must not be retried");
}

#[tokio::test]
async fn failed_task_workspace_probe_leaves_the_starting_candidate_owned() {
    let rig = Rig::prepared().await;
    rig.set_mode("fail-task-probe");
    assert!(rig.start().await.is_err());
    assert_eq!(rig.run_count(), 1);
    assert!(rig.log().contains("/state/work"));
    assert!(rig.log().contains("/workspace"));
    let journal_path = rig
        .task_tree
        .parent()
        .expect("task directory")
        .join("journal.json");
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path).expect("journal"))
            .expect("journal JSON");
    assert_eq!(journal["phase"], "starting");
}

#[tokio::test]
async fn a_running_task_root_with_broadened_permissions_is_refused() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("first start");
    let original_log = rig.log();
    std::fs::set_permissions(&rig.task_tree, std::fs::Permissions::from_mode(0o755))
        .expect("broaden tree-root mode");

    assert!(rig.start().await.is_err());
    assert_eq!(rig.log(), original_log);
}

#[tokio::test]
async fn another_same_generation_container_blocks_task_start() {
    let rig = Rig::prepared().await;
    rig.set_other_container_names(&["renamed-generation-container"]);

    assert!(rig.start().await.is_err());
    assert_eq!(rig.run_count(), 0);
}

#[tokio::test]
async fn a_legacy_same_generation_container_is_not_adopted() {
    let rig = Rig::prepared().await;
    rig.mark_container_exists();
    let mut inspect = rig.inspect_value();
    inspect["Config"]["Labels"]
        .as_object_mut()
        .expect("labels")
        .remove("df.native_task_id");
    rig.set_inspect(inspect);

    assert!(rig.start().await.is_err());
    assert_eq!(rig.run_count(), 0);
}
