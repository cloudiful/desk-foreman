//! Lease-free resource workspace binding resolution cases (issue 815 P1).
//!
//! Every case drives the real internal HTTP router against an isolated
//! per-test PostgreSQL database and a temporary workspace root, so the
//! create-on-miss resolver is exercised end to end with no Docker, no runner
//! and no shared database. Ownership authority stays outside these endpoints:
//! the legacy lease routes and native admission are set up through their own
//! real entry points and then proven unchanged by a resolution.

use serde_json::{Value, json};
use uuid::Uuid;

use desk_foreman::db::{execution_environments::find_native_task, queries};

use super::http::http_post;
use super::resource_binding_fixtures::{
    BindingFixture, RESOURCE_MARKER, assert_lease_free, instant,
};

#[tokio::test]
async fn unauthenticated_callers_are_denied_without_creating_a_binding() {
    let fixture = BindingFixture::setup().await;

    let (status, _) = fixture.resolve(None, "code_project", "alpha").await;
    assert_eq!(status, 401, "a missing application bearer must be rejected");

    let (status, _) = fixture
        .resolve(Some("dfm-not-a-real-token"), "code_project", "alpha")
        .await;
    assert_eq!(status, 401, "an unknown bearer must be rejected");

    assert!(
        fixture
            .resource_row("code_project", "alpha")
            .await
            .is_none(),
        "a denied resolution must not create a resource binding"
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn application_bearer_resolves_one_stable_lease_free_binding() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();

    let (status, created) = fixture.resolve(Some(&token), "code_project", "alpha").await;
    assert_eq!(status, 200);
    assert_eq!(created["application_id"], fixture.application_id);
    assert_eq!(created["resource_kind"], "code_project");
    assert_eq!(created["resource_id"], "alpha");
    assert_eq!(created["workspace_key"], "code_project:alpha");
    assert_eq!(created["external_user_id"], RESOURCE_MARKER);
    assert_eq!(created["lifecycle_state"], "active");
    assert_eq!(created["is_active"], true);
    assert_eq!(created["write_lease_owner"], Value::Null);
    assert_eq!(created["write_lease_acquired_at"], Value::Null);
    assert_eq!(created["write_lease_expires_at"], Value::Null);
    let binding_id = created["workspace_binding_id"]
        .as_i64()
        .expect("binding id");

    let (status, replayed) = fixture.resolve(Some(&token), "code_project", "alpha").await;
    assert_eq!(status, 200);
    assert_eq!(
        replayed["workspace_binding_id"], binding_id,
        "a replay of the same application, kind and id must resolve the same binding"
    );
    assert_eq!(replayed["write_lease_owner"], Value::Null);

    assert_lease_free(&fixture.row(binding_id).await);

    fixture.teardown().await;
}

#[tokio::test]
async fn distinct_applications_resolve_separate_bindings_for_one_resource() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();
    let (other_application_id, other_token) = fixture.other_application("binding-other").await;

    let (status, mine) = fixture
        .resolve(Some(&token), "code_project", "shared")
        .await;
    assert_eq!(status, 200);
    let (status, theirs) = fixture
        .resolve(Some(&other_token), "code_project", "shared")
        .await;
    assert_eq!(status, 200);

    assert_eq!(mine["application_id"], fixture.application_id);
    assert_eq!(theirs["application_id"], other_application_id);
    assert_eq!(mine["workspace_key"], theirs["workspace_key"]);
    assert_ne!(
        mine["workspace_binding_id"], theirs["workspace_binding_id"],
        "one resource resolves to a separate binding per application"
    );

    // Each application still sees only its own binding on a replay.
    let (status, mine_again) = fixture
        .resolve(Some(&token), "code_project", "shared")
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        mine_again["workspace_binding_id"],
        mine["workspace_binding_id"]
    );
    let (_, theirs_again) = fixture
        .resolve(Some(&other_token), "code_project", "shared")
        .await;
    assert_eq!(
        theirs_again["workspace_binding_id"], theirs["workspace_binding_id"],
        "resolving one application must never hand over the other's binding"
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn invalid_resource_components_are_denied_without_a_new_row() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();
    let too_long = "a".repeat(129);

    for (kind, id) in [
        ("code.project", "alpha"),
        ("code_project", "alpha.beta"),
        ("code_project", too_long.as_str()),
    ] {
        let (status, _) = fixture.resolve(Some(&token), kind, id).await;
        assert_eq!(status, 400, "{kind}/{id} must be rejected");
        assert!(
            fixture.resource_row(kind, id).await.is_none(),
            "a rejected resource key must not create a row"
        );
    }

    fixture.teardown().await;
}

#[tokio::test]
async fn any_valid_resource_kind_resolves_but_stays_native_ineligible() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();
    fixture.online_manager("binding-kind-manager").await;

    let (status, body) = fixture.resolve(Some(&token), "prompt_set", "beta").await;
    assert_eq!(status, 200);
    assert_eq!(body["resource_kind"], "prompt_set");
    assert_eq!(body["resource_id"], "beta");
    let binding_id = body["workspace_binding_id"].as_i64().expect("binding id");
    assert_lease_free(&fixture.row(binding_id).await);

    // An arbitrary kind resolves, but it never becomes native-task eligible.
    let refused = fixture
        .admit(binding_id, "prompt-subject", Uuid::new_v4())
        .await
        .expect_err("a non code_project resource must not admit a native task");
    assert_eq!(refused.code(), "binding_ineligible");

    fixture.teardown().await;
}

#[tokio::test]
async fn archived_binding_is_rejected_and_never_resurrected() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();

    let (status, created) = fixture
        .resolve(Some(&token), "code_project", "retired")
        .await;
    assert_eq!(status, 200);
    let binding_id = created["workspace_binding_id"]
        .as_i64()
        .expect("binding id");
    queries::set_workspace_binding_state(fixture.pool(), binding_id, "archived")
        .await
        .expect("archive binding")
        .expect("archive binding row");

    let (status, _) = fixture
        .resolve(Some(&token), "code_project", "retired")
        .await;
    assert_eq!(
        status, 401,
        "the existing active-lifecycle policy must refuse an archived binding"
    );

    let stored = fixture.row(binding_id).await;
    assert_eq!(stored.lifecycle_state, "archived");
    assert_eq!(stored.is_active, false);
    assert_lease_free(&stored);

    fixture.teardown().await;
}

#[tokio::test]
async fn live_legacy_lease_survives_binding_resolution() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();

    let (status, created) = fixture
        .resolve(Some(&token), "code_project", "leased")
        .await;
    assert_eq!(status, 200);
    let binding_id = created["workspace_binding_id"]
        .as_i64()
        .expect("binding id");

    let (status, leased) = http_post(
        &fixture.base,
        &format!("/api/internal/workspace-bindings/{binding_id}/lease"),
        Some(&token),
        json!({"owner": "legacy-writer", "ttl_seconds": 600}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(leased["write_lease_owner"], "legacy-writer");

    let (status, resolved) = fixture
        .resolve(Some(&token), "code_project", "leased")
        .await;
    assert_eq!(status, 200);
    assert_eq!(resolved["workspace_binding_id"], binding_id);
    assert_eq!(
        resolved["write_lease_owner"], leased["write_lease_owner"],
        "resolution must not replace or clear the lease owner"
    );
    assert_eq!(
        resolved["write_lease_acquired_at"], leased["write_lease_acquired_at"],
        "resolution must not renew the lease acquisition"
    );
    assert_eq!(
        resolved["write_lease_expires_at"], leased["write_lease_expires_at"],
        "resolution must not extend the lease expiry"
    );

    let stored = fixture.row(binding_id).await;
    assert_eq!(
        stored.write_lease_owner.as_deref(),
        Some("legacy-writer"),
        "the stored owner must be untouched"
    );
    assert_eq!(
        stored.write_lease_acquired_at,
        instant(&leased["write_lease_acquired_at"]),
        "the stored acquisition stamp must be untouched"
    );
    assert_eq!(
        stored.write_lease_expires_at,
        instant(&leased["write_lease_expires_at"]),
        "the stored expiry must be untouched"
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn native_active_binding_survives_binding_resolution() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();
    fixture.online_manager("binding-native-manager").await;

    let (status, created) = fixture
        .resolve(Some(&token), "code_project", "native")
        .await;
    assert_eq!(status, 200);
    let binding_id = created["workspace_binding_id"]
        .as_i64()
        .expect("binding id");

    let task_id = Uuid::new_v4();
    let admitted = fixture
        .admit(binding_id, "native-subject", task_id)
        .await
        .expect("admit native task");
    let environment_id = admitted.environment_id;

    let (status, resolved) = fixture
        .resolve(Some(&token), "code_project", "native")
        .await;
    assert_eq!(status, 200);
    assert_eq!(resolved["workspace_binding_id"], binding_id);
    assert_eq!(
        resolved["write_lease_owner"],
        Value::Null,
        "resolution must not take over native ownership with a legacy lease"
    );
    assert_lease_free(&fixture.row(binding_id).await);

    let still_active = find_native_task(
        fixture.pool(),
        fixture.application_id,
        environment_id,
        task_id,
    )
    .await
    .expect("the native task must still be readable");
    assert!(
        still_active.is_active(),
        "resolution must not disturb the active native task"
    );

    // Native admission remains the authority over exclusivity.
    let refused = fixture
        .admit(binding_id, "native-subject", Uuid::new_v4())
        .await
        .expect_err("a second task must not share the binding");
    assert_eq!(refused.code(), "task_already_active");

    fixture.teardown().await;
}

#[tokio::test]
async fn concurrent_first_resolution_returns_one_binding_id() {
    let fixture = BindingFixture::setup().await;
    let token = fixture.token.clone();

    let outcomes = tokio::join!(
        fixture.resolve(Some(&token), "code_project", "raced"),
        fixture.resolve(Some(&token), "code_project", "raced"),
        fixture.resolve(Some(&token), "code_project", "raced"),
        fixture.resolve(Some(&token), "code_project", "raced"),
    );
    let ids: Vec<Value> = [outcomes.0, outcomes.1, outcomes.2, outcomes.3]
        .into_iter()
        .map(|(status, body)| {
            assert_eq!(status, 200);
            body["workspace_binding_id"].clone()
        })
        .collect();
    assert!(
        ids.iter().all(|id| id == &ids[0]),
        "concurrent create-on-miss must converge on one binding id, got {ids:?}"
    );

    let stored = fixture.row(ids[0].as_i64().expect("binding id")).await;
    assert_eq!(stored.application_id, fixture.application_id);
    assert_eq!(stored.workspace_key, "code_project:raced");
    assert_lease_free(&stored);

    fixture.teardown().await;
}
