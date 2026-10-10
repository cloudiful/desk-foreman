//! Native-task preparation authorization and ack-consistency cases
//! (issue 753 P2b).
//!
//! What a bearer may reach, and which acknowledgements a preparation will
//! accept. A foreign manager or an acknowledgement that does not match the
//! sealed seed mutates nothing.

use super::prepare_helpers::{admitted_and_seeded, prepare_job, task_state};
use super::support::{TaskFixture, call};

#[tokio::test]
async fn preparation_is_application_and_bearer_scoped() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-scope").await;

    // No bearer.
    let no_bearer = call(
        &fixture.base,
        "POST",
        &fixture.prepare_path(task_id),
        None,
        &[],
        serde_json::to_vec(
            &serde_json::json!({ "expected_generation": 4, "operation_token": "prep-x" }),
        )
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_eq!(no_bearer.status, 401);

    // A foreign application sees nothing.
    let foreign = fixture
        .json_post(
            &fixture.other_application_token,
            &fixture.prepare_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4, "operation_token": "prep-foreign" }),
        )
        .await;
    assert_eq!(foreign.status, 404);

    // The read path is scoped the same way.
    assert_eq!(
        fixture
            .get(
                &fixture.other_application_token,
                &fixture.prepare_path(task_id)
            )
            .await
            .status,
        404
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_manager_cannot_complete_a_preparation() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-foreign-mgr").await;
    assert_eq!(
        fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(task_id),
                4,
                serde_json::json!({ "expected_generation": 4, "operation_token": "prep-fm" }),
            )
            .await
            .status,
        200
    );
    let (job_id, _, _) = prepare_job(&fixture, task_id).await;
    // A registered but different manager cannot complete this job. Its token is
    // minted for a second manager, so the route authenticates it and then the
    // dispatch refuses the foreign assignment.
    let (other_manager_id, other_token) =
        super::support::create_manager(&fixture.database.pool, "other-prep-manager").await;
    assert_ne!(other_manager_id, fixture.manager_id);
    let ack = call(
        &fixture.base,
        "POST",
        "/api/internal/runner-manager/jobs/result",
        Some(&other_token),
        &[],
        serde_json::to_vec(&serde_json::json!({
            "job_id": format!("envjob:{job_id}"),
            "ok": true,
            "result": {
                "task_id": task_id,
                "manifest_sha256": "0".repeat(64),
                "file_count": 0,
                "total_bytes": 0,
                "prepared": true,
            },
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_ne!(
        ack.status,
        204,
        "a foreign manager must not complete: {}",
        ack.body_text()
    );
    assert_eq!(task_state(&fixture, task_id).await, "preparing");
    let (_, state, _) = prepare_job(&fixture, task_id).await;
    assert_eq!(
        state, "pending",
        "a foreign manager cannot complete the job"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_mismatched_prepared_ack_is_refused_and_mutates_nothing() {
    let fixture = TaskFixture::setup().await;
    let task_id = admitted_and_seeded(&fixture, "op-badack").await;
    assert_eq!(
        fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(task_id),
                4,
                serde_json::json!({ "expected_generation": 4, "operation_token": "prep-bad" }),
            )
            .await
            .status,
        200
    );
    let (job_id, _, _) = prepare_job(&fixture, task_id).await;
    sqlx::query("UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = NOW() WHERE job_id = $1")
        .bind(job_id)
        .execute(&fixture.database.pool)
        .await
        .expect("claim job");

    // A wrong digest and a `prepared: false` ack are both refused.
    for (hash, prepared) in [("0".repeat(64), true), ("0".repeat(64), false)] {
        let ack = fixture
            .manager_result(
                job_id,
                true,
                serde_json::json!({
                    "task_id": task_id,
                    "manifest_sha256": hash,
                    "file_count": 2,
                    "total_bytes": 2,
                    "prepared": prepared,
                }),
            )
            .await;
        assert_eq!(ack.status, 400, "body: {}", ack.body_text());
    }
    assert_eq!(task_state(&fixture, task_id).await, "preparing");
    fixture.teardown().await;
}
