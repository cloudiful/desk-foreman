//! Fence and tenancy refusal cases (issue 742 P3): wrong tenant, stale
//! generation, stale proof, conflicting token, wrong ack and unknown fields
//! all refuse without mutating.

use serde_json::json;

use super::helpers::{create_app_with_token, test_state};
use super::http::{http_get, http_post, spawn_http};
use super::requests::{provision, replace, status, stop, wire_job_id};
use super::support::{
    LifecycleDb, bring_manager_online, create_environment, create_manager, ensure_ack, poll_job,
    report, stop_ack,
};

#[tokio::test]
async fn wrong_tenant_cannot_read_or_mutate() {
    let db = LifecycleDb::setup().await;
    let (_, owner_token) = create_app_with_token(&db.pool, "tenant-owner").await;
    let (_, other_token) = create_app_with_token(&db.pool, "tenant-other").await;
    let (_, mgr_token) = create_manager(&db.pool, "tenant-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &owner_token, "tenant-user").await;

    let (status_code, _) = provision(&base, &other_token, env, 0, "tenant-prov").await;
    assert_eq!(status_code, 404, "a foreign application must not provision");
    let (status_code, _) = status(&base, &other_token, env).await;
    assert_eq!(
        status_code, 404,
        "a foreign application must not read status"
    );
    let (status_code, _) = stop(&base, &other_token, env, 0, "tenant-stop").await;
    assert_eq!(status_code, 404, "a foreign application must not stop");

    // The owner still sees an untouched environment.
    let (_, desc) = status(&base, &owner_token, env).await;
    assert_eq!(
        desc["state"], "starting",
        "no foreign mutation may have run"
    );

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn stale_generation_and_proof_are_refused() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "stale-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "stale-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "stale-user").await;

    let (status_code, _) = provision(&base, &app_token, env, 99, "stale-prov").await;
    assert_eq!(status_code, 409, "a stale provision fence must be refused");

    provision(&base, &app_token, env, 0, "stale-prov-ok").await;
    let job = poll_job(&base, &mgr_token).await;
    report(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        true,
        Some(ensure_ack(env, 0)),
        None,
        204,
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");

    let (status_code, _) = stop(&base, &app_token, env, 99, "stale-stop").await;
    assert_eq!(status_code, 409, "a stale stop fence must be refused");

    let (_, desc) = stop(&base, &app_token, env, 0, "stale-stop-ok").await;
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;
    report(
        &base,
        &mgr_token,
        &wire_job_id(&stop_job),
        true,
        Some(stop_ack(env, 0, "exited")),
        None,
        204,
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");

    // A wrong proof job id must be refused.
    let wrong_proof = uuid::Uuid::new_v4();
    let (status_code, _) = replace(
        &base,
        &app_token,
        env,
        1,
        0,
        &wrong_proof.to_string(),
        "stale-replace",
    )
    .await;
    assert_eq!(status_code, 409, "a stale proof must be refused");

    // The valid proof still works and the environment is unchanged by refusals.
    let (status_code, _) = replace(&base, &app_token, env, 1, 0, &stop_job, "fresh-replace").await;
    assert_eq!(status_code, 200);

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn conflicting_token_and_wrong_ack_are_refused() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "token-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "token-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "token-user").await;

    provision(&base, &app_token, env, 0, "shared-token").await;
    let job = poll_job(&base, &mgr_token).await;
    let job_id = job["job_id"].as_str().unwrap().to_string();

    // A wrong ack (foreign generation) is refused by the result transport and
    // mutates nothing.
    let bad_owner = json!({
        "owner": {"ExecutionEnvironment": {
            "environment_id": env.to_string(),
            "generation": 7,
        }},
        "container_name": runner_protocol::RunnerOwner::ExecutionEnvironment {
            environment_id: env.to_string(),
            generation: 7,
        }
        .container_name(),
        "state": "running",
    });
    report(&base, &mgr_token, &job_id, true, Some(bad_owner), None, 400).await;

    // The same token cannot admit a different intent (an ensure token reused
    // for a stop).
    let (status_code, _) = stop(&base, &app_token, env, 0, "shared-token").await;
    assert_eq!(
        status_code, 409,
        "a token with a different intent must conflict"
    );

    // The misused token left no stop intent: a fresh stop works.
    let (status_code, _) = stop(&base, &app_token, env, 0, "fresh-stop-token").await;
    assert_eq!(status_code, 200);

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn unknown_fields_and_missing_auth_are_denied() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "fields-app").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    let env = create_environment(&base, &app_token, "fields-user").await;

    // Unknown field rejected before any mutation.
    let (status_code, _) = http_post(
        &base,
        &format!("/api/internal/execution-environments/{env}/provision"),
        Some(&app_token),
        json!({
            "expected_generation": 0,
            "operation_token": "fields-prov",
            "application_id": 99,
        }),
    )
    .await;
    assert_eq!(
        status_code, 400,
        "a request-supplied application_id must not route"
    );

    // Missing auth.
    let (status_code, _) = http_get(
        &base,
        &format!("/api/internal/execution-environments/{env}/status"),
        None,
    )
    .await;
    assert_eq!(status_code, 401);

    // A blank token is rejected.
    let (status_code, _) = http_post(
        &base,
        &format!("/api/internal/execution-environments/{env}/provision"),
        Some(&app_token),
        json!({"expected_generation": 0, "operation_token": "   "}),
    )
    .await;
    assert_eq!(status_code, 400, "a blank operation token must be rejected");

    // Read-only status did not create anything.
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "starting");
    assert!(desc["operation"].is_null());

    handle.abort();
    db.teardown().await;
}
