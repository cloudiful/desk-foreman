//! HTTP auth and contract cases for execution environments.
//!
//! Proves bearer gating, application-spoof rejection, unknown-field
//! denial for mount/volume/address inputs, the `false` capability flag,
//! and OpenAPI coverage without any Docker or provisioning call.

use serde_json::json;

use super::helpers::{IsolatedDb, create_app_with_token, test_state};
use super::http::{http_get, http_post, spawn_http};

#[tokio::test]
async fn unauthenticated_callers_are_denied() {
    let db = IsolatedDb::setup().await;
    let state = test_state(db.pool.clone());
    let (base, handle) = spawn_http(state).await;

    let (status, _) = http_post(
        &base,
        "/api/internal/execution-environments/ensure",
        None,
        json!({"external_user_id": "u1", "runtime_kind": "opencode"}),
    )
    .await;
    assert_eq!(status, 401, "unauthenticated ensure must be 401");

    let (status, _) = http_post(
        &base,
        "/api/internal/execution-environments/00000000-0000-0000-0000-000000000000/invalidate",
        None,
        json!({"expected_generation": 0}),
    )
    .await;
    assert_eq!(status, 401, "unauthenticated invalidate must be 401");

    let (status, _) = http_get(&base, "/api/internal/application/capabilities", None).await;
    assert_eq!(status, 401, "unauthenticated capabilities must be 401");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn request_application_spoofing_does_not_route() {
    let db = IsolatedDb::setup().await;
    let (app_id, token) = create_app_with_token(&db.pool, "spoof-app").await;
    let state = test_state(db.pool.clone());
    let (base, handle) = spawn_http(state).await;

    let (status, _) = http_post(
        &base,
        "/api/internal/execution-environments/ensure",
        Some(&token),
        json!({
            "external_user_id": "spoof-user",
            "runtime_kind": "opencode",
            "application_id": 999999,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "request-supplied application_id must not route"
    );

    let (status, body) = http_post(
        &base,
        "/api/internal/execution-environments/ensure",
        Some(&token),
        json!({"external_user_id": "spoof-user", "runtime_kind": "opencode"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["application_id"], app_id);

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn mount_and_volume_fields_are_not_supplied() {
    let db = IsolatedDb::setup().await;
    let (_, token) = create_app_with_token(&db.pool, "mount-guard").await;
    let state = test_state(db.pool.clone());
    let (base, handle) = spawn_http(state).await;

    for extra in [
        json!({"external_user_id": "u1", "runtime_kind": "opencode", "state_volume_ref": "df-env-evil"}),
        json!({"external_user_id": "u1", "runtime_kind": "opencode", "mount_path": "/tmp/evil"}),
        json!({"external_user_id": "u1", "runtime_kind": "opencode", "container_address": "10.0.0.1:9999"}),
    ] {
        let (status, _) = http_post(
            &base,
            "/api/internal/execution-environments/ensure",
            Some(&token),
            extra.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "mount/volume/address input must be rejected: {extra}"
        );
    }

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn http_ensure_and_invalidate_follow_fencing_semantics() {
    let db = IsolatedDb::setup().await;
    let (_, token) = create_app_with_token(&db.pool, "http-fence").await;
    let (_, other_token) = create_app_with_token(&db.pool, "http-fence-other").await;
    let state = test_state(db.pool.clone());
    let (base, handle) = spawn_http(state).await;

    let (status, env) = http_post(
        &base,
        "/api/internal/execution-environments/ensure",
        Some(&token),
        json!({"external_user_id": "http-user", "runtime_kind": "opencode"}),
    )
    .await;
    assert_eq!(status, 200);
    let env_id = env["environment_id"]
        .as_str()
        .expect("environment_id")
        .to_string();
    assert_eq!(env["generation"], 0);

    let (status, again_body) = http_post(
        &base,
        "/api/internal/execution-environments/ensure",
        Some(&token),
        json!({"external_user_id": "http-user", "runtime_kind": "opencode"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(again_body["environment_id"], env_id);

    let (status, _) = http_post(
        &base,
        "/api/internal/execution-environments/ensure",
        Some(&token),
        json!({"external_user_id": "http-user", "runtime_kind": "other"}),
    )
    .await;
    assert_eq!(status, 409);

    let (status, invalidated) = http_post(
        &base,
        &format!("/api/internal/execution-environments/{env_id}/invalidate"),
        Some(&token),
        json!({"expected_generation": 0}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(invalidated["generation"], 1);
    assert_eq!(invalidated["state"], "stopping");

    let (status, _) = http_post(
        &base,
        &format!("/api/internal/execution-environments/{env_id}/invalidate"),
        Some(&token),
        json!({"expected_generation": 0}),
    )
    .await;
    assert_eq!(status, 409);

    let (status, _) = http_post(
        &base,
        &format!("/api/internal/execution-environments/{env_id}/invalidate"),
        Some(&other_token),
        json!({"expected_generation": 1}),
    )
    .await;
    assert_eq!(status, 404);

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn capabilities_keep_ready_false_and_openapi_covers_routes() {
    let db = IsolatedDb::setup().await;
    let (_, token) = create_app_with_token(&db.pool, "caps-app").await;
    let state = test_state(db.pool.clone());
    let (base, handle) = spawn_http(state).await;

    let (status, body) = http_get(
        &base,
        "/api/internal/application/capabilities",
        Some(&token),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["execution_environments_ready"], false);

    let document = desk_foreman::api::openapi_document();
    let json = serde_json::to_value(&document).expect("openapi json");
    let paths = json["paths"].as_object().expect("paths");
    assert!(
        paths.contains_key("/api/internal/execution-environments/ensure"),
        "openapi must describe ensure"
    );
    assert!(
        paths.contains_key("/api/internal/execution-environments/{environment_id}/invalidate"),
        "openapi must describe invalidate"
    );
    let schemas = json["components"]["schemas"].as_object().expect("schemas");
    for name in [
        "ExecutionEnvironment",
        "EnsureExecutionEnvironmentRequest",
        "InvalidateExecutionEnvironmentRequest",
    ] {
        assert!(schemas.contains_key(name), "openapi must include {name}");
    }
    let text = serde_json::to_string(&json).expect("openapi text");
    for banned in ["OpenCode", "opencode_form", " ACP ", "AG-UI", "ag-ui"] {
        assert!(
            !text.contains(banned),
            "DF openapi must stay agent-neutral, found {banned}"
        );
    }

    handle.abort();
    db.teardown().await;
}
