//! Concurrent token-race regressions (issue 753 P1r).
//!
//! Admissions that collide on `operation_token` must resolve to exactly one
//! winner and a typed 409 for every loser — never a 500 and never a leak of the
//! winner's identity. The collision is on the token alone, so the racers
//! legitimately target different environments and bindings, and only the token
//! unique index can serialise them.
//!
//! Several racers are released at once so at least two pass the pre-insert
//! replay check together and actually reach the insert conflict; a two-racer
//! test that the runtime happened to serialise would not exercise that path.

use uuid::Uuid;

use super::support::{
    HttpOutcome, TaskFixture, call, create_code_project_binding, create_environment, fence_header,
    set_idle_fence,
};

/// One racer's target: an environment and a workspace binding it may admit into.
#[derive(Clone)]
struct Target {
    environment_id: Uuid,
    binding_id: i64,
    subject: String,
}

/// Build `count` independent targets for one application.
///
/// The first is the fixture's own; each extra one is a fresh environment bound
/// to the same manager with its own code-project workspace, so the only thing
/// the racers share is the operation token.
async fn targets(fixture: &TaskFixture, count: usize) -> Vec<Target> {
    let mut targets = vec![Target {
        environment_id: fixture.environment_id,
        binding_id: fixture.binding_id,
        subject: "task-user".to_string(),
    }];
    for index in 1..count {
        let subject = format!("race-user-{index}");
        let environment_id =
            create_environment(&fixture.database.pool, fixture.application_id, &subject).await;
        set_idle_fence(
            &fixture.database.pool,
            environment_id,
            fixture.manager_id,
            4,
        )
        .await;
        let binding_id = create_code_project_binding(
            &fixture.database.pool,
            fixture.application_id,
            &format!("/tmp/race-workspace-{index}"),
            &format!("code_project:race-{index}"),
        )
        .await;
        targets.push(Target {
            environment_id,
            binding_id,
            subject,
        });
    }
    targets
}

/// One admit for an explicit target on the fixture's own application.
async fn admit_on(
    fixture: &TaskFixture,
    target: &Target,
    task_id: Uuid,
    token: &str,
) -> HttpOutcome {
    call(
        &fixture.base,
        "POST",
        &format!(
            "/api/internal/execution-environments/{}/tasks/admit",
            target.environment_id
        ),
        Some(&fixture.application_token),
        &[fence_header(4)],
        serde_json::to_vec(&serde_json::json!({
            "task_id": task_id,
            "workspace_binding_id": target.binding_id,
            "subject_external_user_id": target.subject,
            "expected_generation": 4,
            "operation_token": token,
        }))
        .expect("admit body")
        .as_slice(),
    )
    .await
}

#[tokio::test]
async fn a_concurrent_same_token_admission_resolves_to_one_winner_and_409s() {
    const RACERS: usize = 4;
    // Repeated so the interleaving is actually exercised, not observed once.
    for _round in 0..3 {
        let fixture = TaskFixture::setup().await;
        let targets = targets(&fixture, RACERS).await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(RACERS));

        // Every racer shares one token but hits its own environment and
        // binding, so only the token unique index can order them.
        let token = "op-concurrent-token";
        let fixture_ref = &fixture;
        let barrier_a = std::sync::Arc::clone(&barrier);
        let barrier_b = std::sync::Arc::clone(&barrier);
        let barrier_c = std::sync::Arc::clone(&barrier);
        let barrier_d = std::sync::Arc::clone(&barrier);
        let task_a = Uuid::new_v4();
        let task_b = Uuid::new_v4();
        let task_c = Uuid::new_v4();
        let task_d = Uuid::new_v4();
        // Clone the targets out so each future owns one and none borrows the
        // vector across the join.
        let target_a = targets[0].clone();
        let target_b = targets[1].clone();
        let target_c = targets[2].clone();
        let target_d = targets[3].clone();
        let (a, b, c, d) = tokio::join!(
            async move {
                barrier_a.wait().await;
                admit_on(fixture_ref, &target_a, task_a, token).await
            },
            async move {
                barrier_b.wait().await;
                admit_on(fixture_ref, &target_b, task_b, token).await
            },
            async move {
                barrier_c.wait().await;
                admit_on(fixture_ref, &target_c, task_c, token).await
            },
            async move {
                barrier_d.wait().await;
                admit_on(fixture_ref, &target_d, task_d, token).await
            }
        );

        let raced = [a, b, c, d];
        let winners = raced.iter().filter(|outcome| outcome.status == 200).count();
        assert_eq!(
            winners,
            1,
            "exactly one winner expected, got {:?}",
            raced.iter().map(|o| o.status).collect::<Vec<_>>()
        );
        // Every loser is a typed conflict, never a 500 and never a leak.
        for loser in raced.iter().filter(|outcome| outcome.status != 200) {
            assert_eq!(
                loser.status,
                409,
                "a losing racer must be a typed conflict, never a 500: {}",
                loser.body_text()
            );
            assert!(
                !loser.body_text().contains(token)
                    && !loser.body_text().contains(&task_a.to_string())
                    && !loser.body_text().contains(&task_b.to_string())
                    && !loser.body_text().contains(&task_c.to_string())
                    && !loser.body_text().contains(&task_d.to_string()),
                "a refusal must not echo the token or a task id: {}",
                loser.body_text()
            );
        }

        // Exactly one durable task row exists for the whole race.
        let rows: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM execution_environment_tasks WHERE operation_token = $1",
        )
        .bind(token)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("count");
        assert_eq!(rows.0, 1, "the token names exactly one intent");
        fixture.teardown().await;
    }
}

#[tokio::test]
async fn a_concurrent_same_task_id_admission_resolves_to_one_winner_and_a_409() {
    // The same race, colliding on the `task_id` primary key instead of the
    // token, must resolve the same way.
    for _round in 0..4 {
        let fixture = TaskFixture::setup().await;
        let (second_env, second_binding) = second_target(&fixture).await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let shared_task = Uuid::new_v4();
        let fixture_ref = &fixture;
        let (first, second) = tokio::join!(
            async move {
                first_barrier.wait().await;
                admit_on(
                    fixture_ref,
                    &Target {
                        environment_id: fixture_ref.environment_id,
                        binding_id: fixture_ref.binding_id,
                        subject: "task-user".to_string(),
                    },
                    shared_task,
                    "op-id-race-first",
                )
                .await
            },
            async move {
                second_barrier.wait().await;
                admit_on(
                    fixture_ref,
                    &Target {
                        environment_id: second_env,
                        binding_id: second_binding,
                        subject: "race-user-second".to_string(),
                    },
                    shared_task,
                    "op-id-race-second",
                )
                .await
            }
        );

        let statuses = [first.status, second.status];
        assert_eq!(
            statuses.iter().filter(|status| **status == 200).count(),
            1,
            "exactly one winner, never two: {statuses:?}"
        );
        let loser = if first.status == 200 { &second } else { &first };
        assert_eq!(
            loser.status,
            409,
            "an id collision must be a typed conflict, never a 500: {}",
            loser.body_text()
        );
        fixture.teardown().await;
    }
}

/// A second environment and workspace for the id-collision race.
async fn second_target(fixture: &TaskFixture) -> (Uuid, i64) {
    let environment_id = create_environment(
        &fixture.database.pool,
        fixture.application_id,
        "race-user-second",
    )
    .await;
    set_idle_fence(
        &fixture.database.pool,
        environment_id,
        fixture.manager_id,
        4,
    )
    .await;
    let binding_id = create_code_project_binding(
        &fixture.database.pool,
        fixture.application_id,
        "/tmp/race-second-workspace",
        "code_project:race-second",
    )
    .await;
    (environment_id, binding_id)
}
