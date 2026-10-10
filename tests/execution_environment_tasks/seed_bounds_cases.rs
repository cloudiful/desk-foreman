//! Seed bounds and path-rule cases (issue 753 P2a).
//!
//! Every case goes through the real HTTP route, so the status a caller sees is
//! the status the handler produced: shape rules are `400`, size ceilings are
//! `413` (including an actually oversized request body), and a NUL content is
//! refused before any SQL.

use uuid::Uuid;

use super::support::{TaskFixture, call};

/// One seeding attempt with explicit path entries, built as raw JSON.
async fn seed_paths(fixture: &TaskFixture, task_id: Uuid, entries: &[(&str, &str)]) -> u16 {
    let files: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, content)| serde_json::json!({ "path": path, "content": content }))
        .collect();
    fixture
        .seed_raw(
            &fixture.application_token,
            task_id,
            serde_json::to_vec(&serde_json::json!({
                "expected_generation": 4,
                "files": files,
            }))
            .expect("body"),
        )
        .await
        .status
}

#[tokio::test]
async fn invalid_paths_are_refused_with_400() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-paths").await;
    for path in [
        "/etc/passwd",
        "../escape",
        "a/../../b",
        "a//b",
        "a/./b",
        "a/",
        "C:/windows",
        "a\\b",
        ".git/config",
        "sub/.GIT/HEAD",
        "a\u{0}b",
        "a\u{7f}b",
    ] {
        assert_eq!(
            seed_paths(&fixture, task_id, &[(path, "x")]).await,
            400,
            "{path:?} must be a 400"
        );
    }
    // Nothing was written by any refused request.
    let rows: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count");
    assert_eq!(rows.0, 0);
    fixture.teardown().await;
}

#[tokio::test]
async fn an_oversized_path_is_refused_with_400() {
    // A path over 512 bytes is a shape rule, so it is a 400; only the request
    // body, the entry count and the content/total sizes are 413.
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-longpath").await;
    let long = "a".repeat(513);
    assert_eq!(seed_paths(&fixture, task_id, &[(&long, "x")]).await, 400);
    fixture.teardown().await;
}

#[tokio::test]
async fn duplicate_and_ancestor_paths_are_refused_with_400() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-dup").await;
    assert_eq!(
        seed_paths(&fixture, task_id, &[("a.txt", "1"), ("a.txt", "2")]).await,
        400
    );
    assert_eq!(
        seed_paths(&fixture, task_id, &[("a", "1"), ("a/b", "2")]).await,
        400
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_nul_content_is_refused_with_400_before_any_sql() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-nul").await;
    // JSON encodes the NUL as an escape, so the request is well-formed.
    assert_eq!(
        seed_paths(&fixture, task_id, &[("a.txt", "a\u{0}b")]).await,
        400
    );
    let rows: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count");
    assert_eq!(rows.0, 0);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_single_content_over_one_mib_is_refused_with_413() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-bigcontent").await;
    let big = "x".repeat(1024 * 1024 + 1);
    assert_eq!(seed_paths(&fixture, task_id, &[("a.txt", &big)]).await, 413);
    fixture.teardown().await;
}

#[tokio::test]
async fn too_many_entries_is_refused_with_413() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-many").await;
    let entries: Vec<(String, String)> = (0..=4096)
        .map(|index| (format!("f{index}.txt"), "x".to_string()))
        .collect();
    let files: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, content)| serde_json::json!({ "path": path, "content": content }))
        .collect();
    let outcome = fixture
        .seed_raw(
            &fixture.application_token,
            task_id,
            serde_json::to_vec(&serde_json::json!({
                "expected_generation": 4,
                "files": files,
            }))
            .expect("body"),
        )
        .await;
    assert_eq!(outcome.status, 413);
    fixture.teardown().await;
}

#[tokio::test]
async fn an_aggregate_over_eight_mib_is_refused_with_413() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-total").await;
    // Nine 1 MiB files exceed the 8 MiB aggregate while each is under its own
    // per-file ceiling.
    let entries: Vec<(String, String)> = (0..9)
        .map(|index| (format!("f{index}.txt"), "x".repeat(1024 * 1024)))
        .collect();
    let files: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, content)| serde_json::json!({ "path": path, "content": content }))
        .collect();
    let outcome = fixture
        .seed_raw(
            &fixture.application_token,
            task_id,
            serde_json::to_vec(&serde_json::json!({
                "expected_generation": 4,
                "files": files,
            }))
            .expect("body"),
        )
        .await;
    assert_eq!(outcome.status, 413, "body head: {}", outcome.body_text());
    fixture.teardown().await;
}

#[tokio::test]
async fn an_oversized_request_body_is_refused_with_413_without_decoding() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-body").await;
    // A body over the 16 MiB transport cap, with a valid Content-Length.
    let filler = "x".repeat(17 * 1024 * 1024);
    let body = serde_json::to_vec(&serde_json::json!({
        "expected_generation": 4,
        "files": [{ "path": "a.txt", "content": filler }],
    }))
    .expect("body");
    let outcome = fixture
        .seed_raw(&fixture.application_token, task_id, body)
        .await;
    assert_eq!(outcome.status, 413, "body head: {}", outcome.body_text());
    fixture.teardown().await;
}

#[tokio::test]
async fn a_malformed_or_unknown_field_body_is_refused_with_400() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-shape").await;

    let malformed = fixture
        .seed_raw(&fixture.application_token, task_id, b"{not json".to_vec())
        .await;
    assert_eq!(malformed.status, 400);

    // Unknown fields are denied at every level.
    let top_level_unknown = call(
        &fixture.base,
        "POST",
        &fixture.seed_path(task_id),
        Some(&fixture.application_token),
        &[super::support::fence_header(0)],
        serde_json::to_vec(&serde_json::json!({
            "expected_generation": 4,
            "files": [],
            "host_path": "/etc/passwd"
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_eq!(top_level_unknown.status, 400);

    let nested_unknown = call(
        &fixture.base,
        "POST",
        &fixture.seed_path(task_id),
        Some(&fixture.application_token),
        &[super::support::fence_header(0)],
        serde_json::to_vec(&serde_json::json!({
            "expected_generation": 4,
            "files": [{ "path": "a.txt", "content": "a", "mode": 493 }]
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_eq!(nested_unknown.status, 400);
    fixture.teardown().await;
}

#[tokio::test]
async fn a_negative_generation_is_refused_with_400() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-negative").await;
    let outcome = call(
        &fixture.base,
        "POST",
        &fixture.seed_path(task_id),
        Some(&fixture.application_token),
        &[super::support::fence_header(0)],
        serde_json::to_vec(&serde_json::json!({
            "expected_generation": -1,
            "files": []
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_eq!(outcome.status, 400);
    fixture.teardown().await;
}
