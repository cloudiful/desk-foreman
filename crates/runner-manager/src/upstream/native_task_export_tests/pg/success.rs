use reqwest::StatusCode;

use super::support::{
    dispatch_export, download, export_file_count, prepared, schedule_export, stop,
};

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory native-task export gate"]
async fn native_task_export_selected_files_flow_through_manager_dispatch_and_download() {
    let rig = prepared(&[("deleted.txt", "old\n"), ("readme.md", "seed\n")]).await;
    std::fs::write(rig.tree_file("readme.md"), "edited by task\n").expect("edit checkout");
    std::fs::write(rig.tree_file("new.txt"), "added by task\n").expect("add checkout file");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        rig.tree_file("new.txt"),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("normal native file mode");
    std::fs::remove_file(rig.tree_file("deleted.txt")).expect("delete seeded checkout file");
    let stop_job_id = stop(&rig, "export-stop-success").await;

    let scheduled = schedule_export(
        &rig,
        stop_job_id,
        "export-selected-success",
        &["new.txt", "deleted.txt", "readme.md"],
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
    assert_eq!(result.job_id, job.job_id);
    let summary = result.result.expect("summary only");
    assert!(summary.get("files").is_none());
    assert_eq!(summary["task_id"], rig.task_id.to_string());
    assert_eq!(
        rig.fake.log(),
        docker_log_before,
        "export capture must not invoke the Docker CLI"
    );
    assert_eq!(export_file_count(&rig, &job).await, 2);
    assert_eq!(
        rig.report_success(&job, summary).await,
        StatusCode::NO_CONTENT,
        "the ordinary manager acknowledgement follows the bounded upload"
    );

    let (status, content) = download(&rig).await;
    assert_eq!(status, StatusCode::OK);
    let content = content.expect("selected export content");
    assert_eq!(content.missing_paths, vec!["deleted.txt"]);
    assert_eq!(
        content
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.content.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("new.txt", "added by task\n"),
            ("readme.md", "edited by task\n"),
        ]
    );
    assert_eq!(rig.fixture.task_state().await, "stopping");
    rig.fixture.db.teardown().await;
}
