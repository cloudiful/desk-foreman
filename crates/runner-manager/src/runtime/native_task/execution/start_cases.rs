use runner_protocol::{SourceSeedSnapshotFile, validate_seed_bundle};

use super::test_support::Rig;

#[tokio::test]
async fn prepared_start_mounts_the_task_and_records_a_running_journal() {
    let rig = Rig::prepared().await;
    let result = rig.start().await.expect("native task start");

    assert_eq!(result.task, rig.request.task);
    assert_eq!(result.environment.owner, rig.request.owner);
    assert_eq!(
        result.environment.container_name,
        rig.request.owner.container_name()
    );
    assert_eq!(result.environment.state, "running");

    let journal_path = rig
        .task_tree
        .parent()
        .expect("task directory")
        .join("journal.json");
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path).expect("read journal"))
            .expect("journal JSON");
    assert_eq!(journal["phase"], "running");

    let log = rig.log();
    assert!(log.contains("volume create"), "{log}");
    assert_eq!(rig.run_count(), 1, "{log}");
    assert!(log.contains(&format!(
        "type=bind,source={},target=/workspace",
        rig.host_tree
    )));
    assert!(log.contains("df-env-123e4567e89b12d3a456426614174000:/state"));
    assert!(log.contains("--workdir /workspace"));
    assert!(log.contains("--workdir /state/work"));
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("exec --user 1000:1000 "))
            .count(),
        2
    );
    assert!(!log.contains("--privileged") && !log.contains("--publish"));
}

#[tokio::test]
async fn running_replay_preserves_edited_task_files_without_recreation() {
    let rig = Rig::prepared().await;
    rig.start().await.expect("first start");
    let task_file = rig.task_tree.join("README.md");
    std::fs::write(&task_file, "edited by the task").expect("edit task file");

    let replay = rig.start().await.expect("running replay");
    assert_eq!(replay.task, rig.request.task);
    assert_eq!(rig.run_count(), 1, "a running replay must not recreate");
    assert_eq!(
        std::fs::read_to_string(&task_file).expect("read edited file"),
        "edited by the task"
    );

    let seed = validate_seed_bundle(&[SourceSeedSnapshotFile::new("README.md", "initial")])
        .expect("original seed");
    let error = rig
        .backend
        .native_tasks
        .materialize(&rig.prepare_request, &seed)
        .await
        .expect_err("a running task tree must not be rematerialized");
    assert!(error.to_string().contains("execution began"), "{error}");
}

#[tokio::test]
async fn concurrent_same_task_starts_create_only_one_container() {
    let rig = Rig::prepared().await;
    let (first, second) = tokio::join!(rig.start(), rig.start());
    assert!(first.is_ok(), "{first:?}");
    assert!(second.is_ok(), "{second:?}");
    assert_eq!(rig.run_count(), 1, "simultaneous starts must serialize");
}
