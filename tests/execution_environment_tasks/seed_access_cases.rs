//! Seed access, fence and terminal cases (issue 753 P2a).
//!
//! Split from `seed_cases.rs` so no test file exceeds the size budget. These
//! cover reading scope, bearer requirements, the generation fence, the
//! candidate/job-reference refusals, and the artifact-history rule: a released
//! or uncertain task keeps its readable snapshot but can never be re-seeded.

use uuid::Uuid;

use super::support::{TaskFixture, call, seed_files};

#[tokio::test]
async fn reading_a_snapshot_requires_the_owning_application() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-read").await;
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a")])
            )
            .await
            .status,
        200
    );

    // The owner reads the contents in canonical order.
    let own = fixture.get_seed(&fixture.application_token, task_id).await;
    assert_eq!(own.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&own.body).expect("json");
    assert_eq!(body["file_count"], 1);
    assert_eq!(body["files"][0]["path"], "a.txt");
    assert_eq!(body["files"][0]["content"], "a");

    // A foreign application cannot see it, and the refusal confirms nothing.
    assert_eq!(
        fixture
            .get_seed(&fixture.other_application_token, task_id)
            .await
            .status,
        404
    );
    // An absent snapshot for an owned task is the same not-found.
    assert_eq!(
        fixture
            .get_seed(&fixture.application_token, Uuid::new_v4())
            .await
            .status,
        404
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn sealing_and_reading_require_a_bearer() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-nobearer").await;

    let body = serde_json::to_vec(&serde_json::json!({
        "expected_generation": 4,
        "files": [],
    }))
    .expect("body");
    let post = call(
        &fixture.base,
        "POST",
        &fixture.seed_path(task_id),
        None,
        &[],
        &body,
    )
    .await;
    assert_eq!(post.status, 401);
    let get = call(
        &fixture.base,
        "GET",
        &fixture.seed_path(task_id),
        None,
        &[],
        b"",
    )
    .await;
    assert_eq!(get.status, 401);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_stale_generation_fence_is_refused() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-stale").await;
    let outcome = fixture
        .seed(
            &fixture.application_token,
            task_id,
            99,
            &seed_files(&[("a.txt", "a")]),
        )
        .await;
    assert_eq!(outcome.status, 409, "body: {}", outcome.body_text());
    fixture.teardown().await;
}

#[tokio::test]
async fn a_candidate_or_job_reference_refuses_the_seal() {
    // A candidate container generation means a writer may exist.
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-candidate").await;
    sqlx::query(
        "UPDATE execution_environment_tasks SET container_generation = 4 WHERE task_id = $1",
    )
    .bind(task_id)
    .execute(&fixture.database.pool)
    .await
    .expect("set candidate");
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a")])
            )
            .await
            .status,
        409
    );
    fixture.teardown().await;

    // A durable job reference means something was already dispatched.
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-jobref").await;
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, native_task_id, deadline_at \
         ) VALUES ($1, $2, $3, $4, $5, 'ensure', 4, 'pending', $6, NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(format!("seed-job-{task_id}"))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(fixture.manager_id)
    .bind(task_id)
    .execute(&fixture.database.pool)
    .await
    .expect("insert job reference");
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a")])
            )
            .await
            .status,
        409
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_released_task_keeps_its_readable_seed_but_cannot_be_reseeded() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-void").await;
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a")])
            )
            .await
            .status,
        200
    );

    // Cancelling a never-started task voids it: the seed is only a DB artifact,
    // so a live writer is still impossible and the release stays allowed.
    let cancel = fixture
        .json_post(
            &fixture.application_token,
            &fixture.cancel_path(task_id),
            4,
            serde_json::json!({ "expected_generation": 4 }),
        )
        .await;
    assert_eq!(cancel.status, 200, "body: {}", cancel.body_text());
    let body: serde_json::Value = serde_json::from_slice(&cancel.body).expect("json");
    assert_eq!(body["state"], "released");

    // The sealed snapshot is artifact history: still readable after the void.
    let read = fixture.get_seed(&fixture.application_token, task_id).await;
    assert_eq!(read.status, 200, "body: {}", read.body_text());
    let content: serde_json::Value = serde_json::from_slice(&read.body).expect("json");
    assert_eq!(content["files"][0]["content"], "a");

    // Sealing again is refused: the task is terminal.
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a")])
            )
            .await
            .status,
        409
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_uncertain_task_keeps_its_readable_seed() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-uncertain").await;
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "a")])
            )
            .await
            .status,
        200
    );
    // A job reference appeared after the seal, so a cancel marks it uncertain.
    sqlx::query(
        "INSERT INTO execution_environment_jobs ( \
             job_id, token, environment_id, application_id, assigned_manager_id, \
             job_kind, target_generation, state, native_task_id, deadline_at \
         ) VALUES ($1, $2, $3, $4, $5, 'ensure', 4, 'pending', $6, NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(format!("uncertain-job-{task_id}"))
    .bind(fixture.environment_id)
    .bind(fixture.application_id)
    .bind(fixture.manager_id)
    .bind(task_id)
    .execute(&fixture.database.pool)
    .await
    .expect("insert job reference");
    assert_eq!(
        fixture
            .json_post(
                &fixture.application_token,
                &fixture.cancel_path(task_id),
                4,
                serde_json::json!({ "expected_generation": 4 }),
            )
            .await
            .status,
        200
    );
    // Still readable as artifact history.
    assert_eq!(
        fixture
            .get_seed(&fixture.application_token, task_id)
            .await
            .status,
        200
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_application_cannot_seal_a_task() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-foreign-seed").await;
    let outcome = fixture
        .seed(
            &fixture.other_application_token,
            task_id,
            4,
            &seed_files(&[("a.txt", "a")]),
        )
        .await;
    assert_eq!(outcome.status, 404, "body: {}", outcome.body_text());
    fixture.teardown().await;
}
