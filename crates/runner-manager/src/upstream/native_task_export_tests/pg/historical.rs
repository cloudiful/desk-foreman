use reqwest::StatusCode;

use super::support::{
    dispatch_export, download, environment_snapshot, prepared, replace_with_ready_successor,
    schedule_export, start, stop,
};

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory native-task export gate"]
async fn native_task_export_keeps_released_checkout_readable_with_successor_ready() {
    let rig = prepared(&[("README.md", "seed\n")]).await;
    start(&rig, "export-start-historical").await;
    std::fs::write(rig.tree_file("README.md"), "retained old task edit\n").expect("edit checkout");
    let stop_job_id = stop(&rig, "export-stop-historical").await;
    replace_with_ready_successor(&rig, stop_job_id).await;
    assert_eq!(rig.fixture.task_state().await, "released");
    let before = environment_snapshot(&rig).await;
    assert_eq!(before.0, 5);
    assert_eq!(before.1, "ready");

    let scheduled = schedule_export(
        &rig,
        stop_job_id,
        "export-historical-replay",
        &["README.md"],
    )
    .await;
    assert_eq!(
        scheduled.status(),
        StatusCode::OK,
        "{}",
        scheduled.text().await.unwrap()
    );
    let job = rig.poll("export_native_task").await;
    let docker_log_before = rig.fake.log();
    let result = dispatch_export(&rig, job.clone()).await;
    assert!(result.ok, "{:?}", result.error);
    let summary = result.result.expect("summary only");
    assert!(summary.get("files").is_none());
    assert_eq!(
        rig.report_success(&job, summary).await,
        StatusCode::NO_CONTENT
    );

    let (status, content) = download(&rig).await;
    assert_eq!(status, StatusCode::OK);
    let content = content.expect("historical export content");
    assert!(content.missing_paths.is_empty());
    assert_eq!(content.files.len(), 1);
    assert_eq!(content.files[0].path, "README.md");
    assert_eq!(content.files[0].content, "retained old task edit\n");
    assert_eq!(rig.fixture.task_state().await, "released");
    assert_eq!(environment_snapshot(&rig).await, before);
    assert_eq!(rig.fake.log(), docker_log_before);
    rig.fixture.db.teardown().await;
}
