use reqwest::StatusCode;

use super::support::{
    dispatch_export, environment_snapshot, export_file_count, export_header_count, prepared,
    schedule_export, stop,
};

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; mandatory native-task export gate"]
async fn native_task_export_unsafe_selection_and_missing_barrier_never_upload() {
    let rig = prepared(&[("README.md", "retained\n")]).await;
    let stop_job_id = stop(&rig, "export-stop-unsafe").await;

    let unsafe_selection = schedule_export(
        &rig,
        stop_job_id,
        "export-unsafe-selection",
        &["../outside"],
    )
    .await;
    assert_eq!(unsafe_selection.status(), StatusCode::BAD_REQUEST);
    assert_eq!(export_header_count(&rig).await, 0);

    let scheduled =
        schedule_export(&rig, stop_job_id, "export-missing-barrier", &["README.md"]).await;
    assert_eq!(scheduled.status(), StatusCode::OK);
    let job = rig.poll("export_native_task").await;
    let before_environment = environment_snapshot(&rig).await;
    let before_task_state = rig.fixture.task_state().await;
    let state_dir = rig
        .fixture
        .root
        .parent()
        .expect("fixture root parent")
        .join("manager-state");
    let barrier = state_dir
        .join(rig.env_id.simple().to_string())
        .join("barrier.json");
    std::fs::remove_file(barrier).expect("remove test task's stop barrier");
    let docker_log_before = rig.fake.log();

    let result = dispatch_export(&rig, job.clone()).await;
    assert!(!result.ok);
    let error = result.error.expect("missing barrier failure");
    assert_eq!(error, "native task export upload failed or refused");
    assert_eq!(rig.fake.log(), docker_log_before);
    assert_eq!(export_file_count(&rig, &job).await, 0);
    assert_eq!(
        rig.report_failure(&job, &error).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rig.fixture.task_state().await, before_task_state);
    assert_eq!(environment_snapshot(&rig).await, before_environment);
    rig.fixture.db.teardown().await;
}
