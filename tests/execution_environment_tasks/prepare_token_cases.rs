//! Opaque token-conflict regressions across preparation and lifecycle jobs.

use uuid::Uuid;

use super::prepare_helpers::{admitted_and_seeded, prepare_job};
use super::support::{
    TaskFixture, call, create_code_project_binding, create_environment, fence_header,
    set_idle_fence,
};

async fn insert_completed_ensure(
    fixture: &TaskFixture,
    application_id: i64,
    environment_id: Uuid,
    token: &str,
) {
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, deadline_at, terminal_at, result \
         ) VALUES ($1, $2, $3, $4, $5, 'ensure', 4, 'succeeded', \
                   NOW() + INTERVAL '1 minute', NOW(), '{\"outcome\":\"ready\"}')",
    )
    .bind(Uuid::new_v4())
    .bind(token)
    .bind(environment_id)
    .bind(application_id)
    .bind(fixture.manager_id)
    .execute(&fixture.database.pool)
    .await
    .expect("insert completed ensure");
}

async fn create_seeded_task(
    fixture: &TaskFixture,
    environment_id: Uuid,
    binding_id: i64,
    subject: &str,
    operation_token: &str,
) -> Uuid {
    let task_id = Uuid::new_v4();
    let task_path =
        format!("/api/internal/execution-environments/{environment_id}/tasks/{task_id}");
    let admitted = call(
        &fixture.base,
        "POST",
        &format!("/api/internal/execution-environments/{environment_id}/tasks/admit"),
        Some(&fixture.application_token),
        &[fence_header(4)],
        serde_json::to_vec(&serde_json::json!({
            "task_id": task_id,
            "workspace_binding_id": binding_id,
            "subject_external_user_id": subject,
            "expected_generation": 4,
            "operation_token": operation_token,
        }))
        .expect("admission body")
        .as_slice(),
    )
    .await;
    assert_eq!(admitted.status, 200, "body: {}", admitted.body_text());

    let seeded = call(
        &fixture.base,
        "POST",
        &format!("{task_path}/seed"),
        Some(&fixture.application_token),
        &[fence_header(4)],
        serde_json::to_vec(&serde_json::json!({
            "expected_generation": 4,
            "files": [
                { "path": "a.txt", "content": "a" },
                { "path": "b.txt", "content": "b" },
            ],
        }))
        .expect("seed body")
        .as_slice(),
    )
    .await;
    assert_eq!(seeded.status, 200, "body: {}", seeded.body_text());
    task_id
}

async fn schedule_preparation(
    fixture: &TaskFixture,
    environment_id: Uuid,
    task_id: Uuid,
    token: &str,
) -> super::support::HttpOutcome {
    call(
        &fixture.base,
        "POST",
        &format!("/api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare"),
        Some(&fixture.application_token),
        &[fence_header(4)],
        serde_json::to_vec(&serde_json::json!({
            "expected_generation": 4,
            "operation_token": token,
        }))
        .expect("prepare body")
        .as_slice(),
    )
    .await
}

#[tokio::test]
async fn sequential_token_reuse_by_other_jobs_is_an_opaque_conflict() {
    let fixture = TaskFixture::setup().await;
    let foreign_app =
        super::helpers::create_app_with_token(&fixture.database.pool, "task-foreign-token-owner")
            .await
            .0;
    let foreign_env =
        create_environment(&fixture.database.pool, foreign_app, "foreign-token-user").await;
    set_idle_fence(&fixture.database.pool, foreign_env, fixture.manager_id, 4).await;
    insert_completed_ensure(
        &fixture,
        fixture.application_id,
        fixture.environment_id,
        "completed-ensure-token",
    )
    .await;
    insert_completed_ensure(&fixture, foreign_app, foreign_env, "foreign-app-token").await;

    let task_a = admitted_and_seeded(&fixture, "op-token-a").await;
    for token in ["completed-ensure-token", "foreign-app-token"] {
        let outcome = schedule_preparation(&fixture, fixture.environment_id, task_a, token).await;
        assert_eq!(outcome.status, 409, "body: {}", outcome.body_text());
        assert!(!outcome.body_text().contains(token));
    }

    let second_env = create_environment(
        &fixture.database.pool,
        fixture.application_id,
        "task-user-second-env",
    )
    .await;
    set_idle_fence(&fixture.database.pool, second_env, fixture.manager_id, 4).await;
    let second_binding = create_code_project_binding(
        &fixture.database.pool,
        fixture.application_id,
        "/workspace/second-native-task",
        "code_project:second-native-task",
    )
    .await;
    let task_b = create_seeded_task(
        &fixture,
        second_env,
        second_binding,
        "task-user-second-env",
        "op-token-b",
    )
    .await;
    let scheduled = schedule_preparation(&fixture, second_env, task_b, "prepared-task-token").await;
    assert_eq!(scheduled.status, 200, "body: {}", scheduled.body_text());
    let (job_b, _, _) = prepare_job(&fixture, task_b).await;
    sqlx::query(
        "UPDATE execution_environment_jobs SET state = 'claimed', claimed_at = NOW() WHERE job_id = $1",
    )
    .bind(job_b)
    .execute(&fixture.database.pool)
    .await
    .expect("claim second preparation");
    let seed: (String, i64, i64) = sqlx::query_as(
        "SELECT manifest_sha256, file_count, total_bytes \
         FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_b)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("second seed");
    let prepared = fixture
        .manager_result(
            job_b,
            true,
            serde_json::json!({
                "task_id": task_b,
                "manifest_sha256": seed.0,
                "file_count": seed.1,
                "total_bytes": seed.2,
                "prepared": true,
            }),
        )
        .await;
    assert_eq!(prepared.status, 204, "body: {}", prepared.body_text());

    let conflict = schedule_preparation(
        &fixture,
        fixture.environment_id,
        task_a,
        "prepared-task-token",
    )
    .await;
    assert_eq!(conflict.status, 409, "body: {}", conflict.body_text());
    assert!(!conflict.body_text().contains(&task_b.to_string()));
    fixture.teardown().await;
}

#[tokio::test]
async fn concurrent_cross_task_token_reuse_has_one_winner_without_500() {
    let fixture = TaskFixture::setup().await;
    let task_a = admitted_and_seeded(&fixture, "op-concurrent-token-a").await;
    let second_env = create_environment(
        &fixture.database.pool,
        fixture.application_id,
        "task-user-concurrent-env",
    )
    .await;
    set_idle_fence(&fixture.database.pool, second_env, fixture.manager_id, 4).await;
    let second_binding = create_code_project_binding(
        &fixture.database.pool,
        fixture.application_id,
        "/workspace/concurrent-native-task",
        "code_project:concurrent-native-task",
    )
    .await;
    let task_b = create_seeded_task(
        &fixture,
        second_env,
        second_binding,
        "task-user-concurrent-env",
        "op-concurrent-token-b",
    )
    .await;

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let first_barrier = std::sync::Arc::clone(&barrier);
    let second_barrier = std::sync::Arc::clone(&barrier);
    let fixture_ref = &fixture;
    let (first, second) = tokio::join!(
        async move {
            first_barrier.wait().await;
            schedule_preparation(
                fixture_ref,
                fixture_ref.environment_id,
                task_a,
                "shared-cross-task-token",
            )
            .await
        },
        async move {
            second_barrier.wait().await;
            schedule_preparation(fixture_ref, second_env, task_b, "shared-cross-task-token").await
        }
    );
    let statuses = [first.status, second.status];
    assert!(statuses.contains(&200), "one task must win: {statuses:?}");
    assert!(
        statuses.contains(&409),
        "the loser must conflict: {statuses:?}"
    );
    assert!(
        !statuses.contains(&500),
        "no uniqueness violation may leak: {statuses:?}"
    );
    fixture.teardown().await;
}
