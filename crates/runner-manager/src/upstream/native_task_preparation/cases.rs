//! Named gate cases for native-task preparation (issue 753 P2b).
//!
//! Real isolated PostgreSQL, the real DF router and the real manager dispatch,
//! with no Docker.

use desk_foreman::db::execution_environments::tasks::seeds::find_native_task_seed;
use runner_protocol::{PrepareNativeTaskRequest, SourceSeedSnapshotFile, validate_seed_bundle};
use uuid::Uuid;

use super::super::native_task as mgr_native_task;
use super::support::{Rig, online_manager, prepare_request};

#[tokio::test]
#[ignore = "requires isolatedPG; mandatory named gate"]
async fn native_task_preparation_poll_fetch_materialize_ack() {
    let files = [
        ("src/main.rs", "fn main() {}\n"),
        ("README.md", "# Héllo\n"),
    ];
    let rig = Rig::start(&files).await;
    let backend = rig.backend(Some(rig.root.clone()));

    let job = rig.poll().await;
    assert_eq!(job.kind, "prepare_native_task");
    let request = prepare_request(&job);
    assert_eq!(request.task_id, rig.task_id);

    // Fetch + materialize through the real dispatch helper and backend.
    let result = mgr_native_task::prepare(
        &rig.client,
        &rig.base,
        &rig.manager_token,
        &rig.manager_id.to_string(),
        &job.job_id,
        request.clone(),
        &*backend,
    )
    .await
    .expect("prepare");
    assert!(result.prepared);

    // Acknowledge as the manager.
    let response = rig
        .client
        .post(format!(
            "{}/api/internal/runner-manager/jobs/result",
            rig.base
        ))
        .bearer_auth(&rig.manager_token)
        .json(&serde_json::json!({
            "job_id": job.job_id,
            "ok": true,
            "result": result,
            "error": null,
        }))
        .send()
        .await
        .expect("result");
    assert_eq!(
        response.status(),
        204,
        "{}",
        response.text().await.unwrap_or_default()
    );

    assert_eq!(rig.task_state().await, "prepared");
    // The environment readiness and candidate are untouched by a
    // preparation.
    let (candidate, state) = rig.environment_fence().await;
    assert_eq!(candidate, None, "a preparation never sets a candidate");
    assert_eq!(
        state, "absent",
        "a preparation never makes an environment ready"
    );

    // The checkout is byte-exact, including non-ASCII UTF-8.
    let tree = rig.tree_dir();
    assert_eq!(
        std::fs::read_to_string(tree.join("src/main.rs")).expect("read"),
        "fn main() {}\n"
    );
    assert_eq!(
        std::fs::read(tree.join("README.md")).expect("read"),
        "# Héllo\n".as_bytes()
    );
    rig.db.teardown().await;
}

/// A foreign manager cannot retrieve another manager's sealed seed.
#[tokio::test]
#[ignore = "requires isolatedPG; mandatory named gate"]
async fn unauthorized_manager_seed_is_404() {
    let rig = Rig::start(&[("a.txt", "a")]).await;
    let job = rig.poll().await;
    let (_other_id, other_token) = online_manager(&rig.db.pool, "other-manager").await;
    let response = rig
        .client
        .get(format!(
            "{}/api/internal/runner-managers/{}/environment-jobs/{}/task-seed",
            rig.base,
            rig.manager_id,
            job.job_id.trim_start_matches("envjob:")
        ))
        .bearer_auth(&other_token)
        .send()
        .await
        .expect("seed");
    assert_eq!(response.status(), 404);
    rig.db.teardown().await;
}

/// A job that is no longer live cannot be retrieved.
#[tokio::test]
#[ignore = "requires isolatedPG; mandatory named gate"]
async fn an_expired_job_retrieval_is_refused() {
    let rig = Rig::start(&[("a.txt", "a")]).await;
    let job = rig.poll().await;
    let job_uuid = job.job_id.trim_start_matches("envjob:").to_string();
    sqlx::query("UPDATE execution_environment_jobs SET state = 'uncertain', terminal_at = NOW() WHERE job_id = $1")
        .bind(Uuid::parse_str(&job_uuid).expect("uuid"))
        .execute(&rig.db.pool)
        .await
        .expect("expire job");
    let response = rig
        .client
        .get(format!(
            "{}/api/internal/runner-managers/{}/environment-jobs/{}/task-seed",
            rig.base, rig.manager_id, job_uuid
        ))
        .bearer_auth(&rig.manager_token)
        .send()
        .await
        .expect("seed");
    assert_eq!(response.status(), 404);
    rig.db.teardown().await;
}

/// A disabled root yields a fixed unsupported failure, and the control
/// plane keeps the task uncertain rather than pretending success.
#[tokio::test]
#[ignore = "requires isolatedPG; mandatory named gate"]
async fn a_disabled_root_cannot_produce_a_false_success() {
    let rig = Rig::start(&[("a.txt", "a")]).await;
    let backend = rig.backend(None);
    let job = rig.poll().await;
    let request: PrepareNativeTaskRequest =
        serde_json::from_value(job.payload.clone()).expect("prepare request");
    let error = mgr_native_task::prepare(
        &rig.client,
        &rig.base,
        &rig.manager_token,
        &rig.manager_id.to_string(),
        &job.job_id,
        request,
        &*backend,
    )
    .await
    .expect_err("disabled root must fail");
    assert!(error.to_string().contains("not configured"), "{error}");

    // Report the failure as the manager would.
    let response = rig
        .client
        .post(format!(
            "{}/api/internal/runner-manager/jobs/result",
            rig.base
        ))
        .bearer_auth(&rig.manager_token)
        .json(&serde_json::json!({
            "job_id": job.job_id,
            "ok": false,
            "result": null,
            "error": "native task preparation is not configured",
        }))
        .send()
        .await
        .expect("result");
    assert_eq!(response.status(), 204);
    assert_eq!(rig.task_state().await, "uncertain");
    // No tree was created anywhere.
    assert!(!rig.root.join(rig.env_id.to_string()).exists());
    rig.db.teardown().await;
}

/// A digest that disagrees with the sealed seed never writes, and a repeat
/// prepared call does not rewrite the tree.
#[tokio::test]
#[ignore = "requires isolatedPG; mandatory named gate"]
async fn mismatched_seed_never_writes_and_repeat_is_stable() {
    let rig = Rig::start(&[("a.txt", "a")]).await;
    let backend = rig.backend(Some(rig.root.clone()));
    let job = rig.poll().await;
    let mut request: PrepareNativeTaskRequest =
        serde_json::from_value(job.payload.clone()).expect("prepare request");
    request.manifest_sha256 = "0".repeat(64);
    assert!(
        mgr_native_task::prepare(
            &rig.client,
            &rig.base,
            &rig.manager_token,
            &rig.manager_id.to_string(),
            &job.job_id,
            request,
            &*backend,
        )
        .await
        .is_err()
    );
    // The trusted tree directory is never created on a mismatch.
    let task_dir = rig
        .root
        .join(rig.env_id.to_string())
        .join(rig.task_id.to_string());
    assert!(!task_dir.join("tree").exists());

    // Now a correct call materializes, and a repeat leaves the inode alone.
    let request: PrepareNativeTaskRequest =
        serde_json::from_value(job.payload.clone()).expect("prepare request");
    mgr_native_task::prepare(
        &rig.client,
        &rig.base,
        &rig.manager_token,
        &rig.manager_id.to_string(),
        &job.job_id,
        request.clone(),
        &*backend,
    )
    .await
    .expect("prepare");
    use std::os::unix::fs::MetadataExt;
    let before = std::fs::symlink_metadata(task_dir.join("tree/a.txt"))
        .expect("stat")
        .ino();
    mgr_native_task::prepare(
        &rig.client,
        &rig.base,
        &rig.manager_token,
        &rig.manager_id.to_string(),
        &job.job_id,
        request,
        &*backend,
    )
    .await
    .expect("replay");
    let after = std::fs::symlink_metadata(task_dir.join("tree/a.txt"))
        .expect("stat")
        .ino();
    assert_eq!(before, after, "a repeat must not rewrite the tree");
    rig.db.teardown().await;
}

/// The fetched seed round-trips through the shared rules unchanged.
#[tokio::test]
#[ignore = "requires isolatedPG; mandatory named gate"]
async fn the_fetched_seed_matches_the_sealed_digest() {
    let rig = Rig::start(&[("a/b.txt", "x"), ("c.txt", "y")]).await;
    let job = rig.poll().await;
    let request: PrepareNativeTaskRequest =
        serde_json::from_value(job.payload).expect("prepare request");
    let job_uuid = Uuid::parse_str(rig.task_id.to_string().as_str())
        .map(|_| ())
        .is_ok();
    let _ = job_uuid;
    let descriptor = find_native_task_seed(&rig.db.pool, rig.app_id, rig.env_id, rig.task_id)
        .await
        .expect("seed");
    assert_eq!(descriptor.manifest_sha256, request.manifest_sha256);
    assert_eq!(descriptor.file_count, request.file_count);
    assert_eq!(descriptor.total_bytes, request.total_bytes);
    // The shared validator agrees with the sealed digest.
    let bundle = validate_seed_bundle(&[
        SourceSeedSnapshotFile::new("a/b.txt", "x"),
        SourceSeedSnapshotFile::new("c.txt", "y"),
    ])
    .expect("bundle");
    assert_eq!(bundle.manifest_sha256, request.manifest_sha256);
    rig.db.teardown().await;
}
