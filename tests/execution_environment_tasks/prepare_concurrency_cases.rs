//! Native-task preparation concurrency cases (issue 753 P2b).
//!
//! A task has at most one durable preparation, no matter how many callers race
//! to schedule it.

use super::prepare_helpers::admitted_and_seeded;
use super::support::TaskFixture;

#[tokio::test]
async fn two_concurrent_preparations_leave_exactly_one_durable_job() {
    for _round in 0..4 {
        let fixture = TaskFixture::setup().await;
        let task_id = admitted_and_seeded(&fixture, "op-race").await;

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let fixture_ref = &fixture;
        let (first, second) = tokio::join!(
            async move {
                first_barrier.wait().await;
                fixture_ref
                    .json_post(
                        &fixture_ref.application_token,
                        &fixture_ref.prepare_path(task_id),
                        4,
                        serde_json::json!({
                            "expected_generation": 4,
                            "operation_token": "prep-race-same",
                        }),
                    )
                    .await
            },
            async move {
                second_barrier.wait().await;
                fixture_ref
                    .json_post(
                        &fixture_ref.application_token,
                        &fixture_ref.prepare_path(task_id),
                        4,
                        serde_json::json!({
                            "expected_generation": 4,
                            "operation_token": "prep-race-same",
                        }),
                    )
                    .await
            }
        );

        let statuses = [first.status, second.status];
        assert_ne!(statuses[0], 500, "never a 500: {statuses:?}");
        assert_ne!(statuses[1], 500, "never a 500: {statuses:?}");
        assert!(
            statuses.contains(&200),
            "at least one must succeed: {statuses:?}"
        );
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM execution_environment_jobs \
             WHERE native_task_id = $1 AND job_kind = 'prepare_native_task'",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("count");
        assert_eq!(count.0, 1, "exactly one durable preparation");
        fixture.teardown().await;
    }
}
