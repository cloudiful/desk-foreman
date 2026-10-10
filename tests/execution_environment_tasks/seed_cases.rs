//! Immutable task input seed cases (issue 753 P2a).
//!
//! Drives the real internal HTTP surface against an isolated PostgreSQL. This
//! file covers sealing: the descriptor, canonical ordering, the empty
//! workspace, idempotent replay and immutability under differing content.
//! Who may seal/read and the terminal fences live in `seed_access_cases.rs`.

use uuid::Uuid;

use super::support::{TaskFixture, seed_files};

#[tokio::test]
async fn a_sealed_snapshot_returns_its_manifest_and_counts() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(fixture.admit_default(task_id, "op-seed").await.status, 200);

    let outcome = fixture
        .seed(
            &fixture.application_token,
            task_id,
            4,
            &seed_files(&[("README.md", "# hi\n"), ("src/main.rs", "fn main() {}\n")]),
        )
        .await;
    assert_eq!(outcome.status, 200, "body: {}", outcome.body_text());
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(body["task_id"], task_id.to_string());
    assert_eq!(body["file_count"], 2);
    assert_eq!(
        body["total_bytes"],
        ("# hi\n".len() + "fn main() {}\n".len()) as i64
    );
    // The published v1 vector for this exact content.
    assert_eq!(
        body["manifest_sha256"],
        "d884c9b8dda5f737e27c9a67d06ee99fcdd85e03e3a7edbb184f8a84d0794a59"
    );
    // The descriptor exposes identity only: no path, credential or host detail.
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["file_count", "manifest_sha256", "task_id", "total_bytes"]
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_unsorted_request_seals_the_same_digest_as_a_sorted_one() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-order").await;
    // Reverse order in the request must not change the manifest.
    let outcome = fixture
        .seed(
            &fixture.application_token,
            task_id,
            4,
            &seed_files(&[("b.txt", "b"), ("a.txt", "a")]),
        )
        .await;
    assert_eq!(outcome.status, 200, "body: {}", outcome.body_text());
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(
        body["manifest_sha256"],
        "20410d9c695bacbbcd273369f4da3274431218c25f1a0975532b9be79e9f393b"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_empty_file_list_seals_a_valid_empty_workspace() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-empty").await;
    let outcome = fixture
        .seed(&fixture.application_token, task_id, 4, &[])
        .await;
    assert_eq!(outcome.status, 200, "body: {}", outcome.body_text());
    let body: serde_json::Value = serde_json::from_slice(&outcome.body).expect("json");
    assert_eq!(body["file_count"], 0);
    assert_eq!(body["total_bytes"], 0);
    assert_eq!(
        body["manifest_sha256"],
        "39ee0d00eb7cf8290dcdc707fcc0c30dec7ccd473023a5a4a38e734151ae35c7"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_exact_seal_replay_returns_the_same_snapshot() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-replay").await;
    let entries = seed_files(&[("a.txt", "a"), ("b.txt", "b")]);
    let first = fixture
        .seed(&fixture.application_token, task_id, 4, &entries)
        .await;
    assert_eq!(first.status, 200);
    // Same content, different request order: still an exact replay.
    let second = fixture
        .seed(
            &fixture.application_token,
            task_id,
            4,
            &seed_files(&[("b.txt", "b"), ("a.txt", "a")]),
        )
        .await;
    assert_eq!(second.status, 200, "body: {}", second.body_text());
    assert_eq!(
        first.body, second.body,
        "a replay returns the same descriptor"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_different_content_seal_is_refused_and_never_overwrites() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    fixture.admit_default(task_id, "op-conflict").await;
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
    // Different content under the same task is a conflict.
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("a.txt", "b")])
            )
            .await
            .status,
        409
    );
    // Different path is also a conflict.
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &seed_files(&[("c.txt", "a")])
            )
            .await
            .status,
        409
    );
    // The original snapshot is intact and still readable.
    let read = fixture.get_seed(&fixture.application_token, task_id).await;
    assert_eq!(read.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&read.body).expect("json");
    let stored: Vec<(String, String)> = body["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| {
            (
                file["path"].as_str().expect("path").to_string(),
                file["content"].as_str().expect("content").to_string(),
            )
        })
        .collect();
    assert_eq!(stored, seed_files(&[("a.txt", "a")]));
    fixture.teardown().await;
}
