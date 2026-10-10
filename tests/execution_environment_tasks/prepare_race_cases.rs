//! Concurrent cancellation and preparation-ack races (issue 753 P3r).

use super::prepare_helpers::{admitted_and_seeded, prepare_job, task_state};
use super::support::TaskFixture;

#[tokio::test]
async fn cancellation_and_success_ack_have_one_serialized_owner_outcome() {
    for round in 0..3 {
        let fixture = TaskFixture::setup().await;
        let task_id = admitted_and_seeded(&fixture, &format!("op-race-{round}")).await;
        let scheduled = fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(task_id),
                4,
                serde_json::json!({
                    "expected_generation": 4,
                    "operation_token": format!("prep-race-{round}"),
                }),
            )
            .await;
        assert_eq!(scheduled.status, 200, "body: {}", scheduled.body_text());
        let (job_id, _, _) = prepare_job(&fixture, task_id).await;
        sqlx::query(
            "UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = NOW() WHERE job_id = $1",
        )
        .bind(job_id)
        .execute(&fixture.database.pool)
        .await
        .expect("claim preparation");
        let seed: (String, i64, i64) = sqlx::query_as(
            "SELECT manifest_sha256, file_count, total_bytes \
             FROM execution_environment_task_seeds WHERE task_id = $1",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("seed header");
        let ack = serde_json::json!({
            "task_id": task_id,
            "manifest_sha256": seed.0,
            "file_count": seed.1,
            "total_bytes": seed.2,
            "prepared": true,
        });

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let ack_barrier = std::sync::Arc::clone(&barrier);
        let cancel_barrier = std::sync::Arc::clone(&barrier);
        let fixture_ref = &fixture;
        let (ack_result, cancel_result) = tokio::join!(
            async move {
                ack_barrier.wait().await;
                fixture_ref.manager_result(job_id, true, ack).await
            },
            async move {
                cancel_barrier.wait().await;
                fixture_ref
                    .json_post(
                        &fixture_ref.application_token,
                        &fixture_ref.cancel_path(task_id),
                        4,
                        serde_json::json!({ "expected_generation": 4 }),
                    )
                    .await
            }
        );
        assert_eq!(
            cancel_result.status,
            200,
            "body: {}",
            cancel_result.body_text()
        );
        assert!(
            ack_result.status == 204 || ack_result.status == 409,
            "ack outcome must be serialized, got {}: {}",
            ack_result.status,
            ack_result.body_text()
        );
        assert_eq!(task_state(&fixture, task_id).await, "uncertain");
        let (_, job_state, _) = prepare_job(&fixture, task_id).await;
        assert!(matches!(job_state.as_str(), "claimed" | "succeeded"));
        if job_state == "succeeded" {
            let result: (serde_json::Value,) =
                sqlx::query_as("SELECT result FROM execution_environment_jobs WHERE job_id = $1")
                    .bind(job_id)
                    .fetch_one(&fixture.database.pool)
                    .await
                    .expect("success result");
            assert_eq!(result.0["outcome"], "task_prepared");
        } else {
            assert_eq!(ack_result.status, 409, "a canceled task cannot complete");
        }
        fixture.teardown().await;
    }
}

#[tokio::test]
async fn expiry_reconciliation_and_late_ack_serialize_on_the_environment() {
    for round in 0..3 {
        let fixture = TaskFixture::setup().await;
        let task_id = admitted_and_seeded(&fixture, &format!("op-expiry-race-{round}")).await;
        let scheduled = fixture
            .json_post(
                &fixture.application_token,
                &fixture.prepare_path(task_id),
                4,
                serde_json::json!({
                    "expected_generation": 4,
                    "operation_token": format!("prep-expiry-race-{round}"),
                }),
            )
            .await;
        assert_eq!(scheduled.status, 200, "body: {}", scheduled.body_text());
        let (job_id, _, _) = prepare_job(&fixture, task_id).await;
        sqlx::query(
            "UPDATE execution_environment_jobs \
             SET state = 'claimed', claimed_at = NOW(), \
                 deadline_at = NOW() - INTERVAL '1 second' WHERE job_id = $1",
        )
        .bind(job_id)
        .execute(&fixture.database.pool)
        .await
        .expect("expire claimed preparation");
        let seed: (String, i64, i64) = sqlx::query_as(
            "SELECT manifest_sha256, file_count, total_bytes \
             FROM execution_environment_task_seeds WHERE task_id = $1",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("seed header");
        let ack = serde_json::json!({
            "task_id": task_id,
            "manifest_sha256": seed.0,
            "file_count": seed.1,
            "total_bytes": seed.2,
            "prepared": true,
        });

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let status_barrier = std::sync::Arc::clone(&barrier);
        let ack_barrier = std::sync::Arc::clone(&barrier);
        let fixture_ref = &fixture;
        let status_path = format!(
            "/api/internal/execution-environments/{}/status",
            fixture.environment_id
        );
        let (status, ack) = tokio::join!(
            async move {
                status_barrier.wait().await;
                fixture_ref
                    .get(&fixture_ref.application_token, &status_path)
                    .await
            },
            async move {
                ack_barrier.wait().await;
                fixture_ref.manager_result(job_id, true, ack).await
            }
        );
        assert_eq!(status.status, 200, "body: {}", status.body_text());
        assert_eq!(ack.status, 409, "body: {}", ack.body_text());
        assert_eq!(task_state(&fixture, task_id).await, "uncertain");
        let (_, job_state, _) = prepare_job(&fixture, task_id).await;
        assert_eq!(job_state, "uncertain");
        fixture.teardown().await;
    }
}
