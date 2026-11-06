use super::*;
use axum::body::to_bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;

use crate::auth::identity::require_isolated_resource_workspace;
use crate::error::AppError;
use crate::lease_test_support as support;

#[tokio::test]
async fn non_isolated_resource_workspace_is_forbidden_403() {
    let error = require_isolated_resource_workspace(false, "artifact:one")
        .expect_err("a non-isolated resource workspace must be rejected");
    let message = match &error {
        AppError::Forbidden(message) => message,
        other => panic!("expected a forbidden error, got {other:?}"),
    };
    assert_eq!(message, "resource workspace requires an isolated principal");

    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("error JSON");
    assert_eq!(
        body,
        serde_json::json!({ "error": "resource workspace requires an isolated principal" })
    );
}

#[test]
fn isolated_resource_workspace_is_allowed() {
    require_isolated_resource_workspace(true, "artifact:one")
        .expect("an isolated resource workspace is allowed");
}

#[test]
fn non_isolated_personal_workspace_is_allowed() {
    require_isolated_resource_workspace(false, "default")
        .expect("a non-isolated personal workspace is allowed");
    require_isolated_resource_workspace(false, "person-a")
        .expect("a non-isolated personal workspace is allowed");
}

#[test]
fn malformed_resource_workspace_keeps_personal_semantics() {
    for workspace_key in ["artifact", "artifact:", "Code:one"] {
        require_isolated_resource_workspace(false, workspace_key).unwrap_or_else(|_| {
            panic!("malformed resource key {workspace_key} must stay personal")
        });
    }
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4l PostgreSQL"]
async fn authentication_carries_checked_principal_without_invalid_binding_creation()
-> anyhow::Result<()> {
    use crate::{
        actor::{McpActor, actor_from_mcp_actor},
        db::{
            queries,
            types::{CreateApplicationTokenRequest, CreateMcpTokenRequest},
        },
    };
    use runner_protocol::workspace_principal::application_principal_owner;
    use std::sync::Arc;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("P4L_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_lifecycle_seed.sql"
    ))
    .execute(&pool)
    .await?;
    let (token, _) = queries::create_application_token(
        &pool,
        &CreateApplicationTokenRequest {
            application_id: 817301,
            token_name: "p4l".into(),
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await?;
    let (user_token, _) = queries::create_mcp_token(
        &pool,
        &CreateMcpTokenRequest {
            user_id: 817301,
            token_name: "p4l-user".into(),
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await?;
    let temp = tempfile::tempdir()?;
    let mut state = support::fake_state(support::FakeRunnerService::new());
    state.db = pool.clone();
    let mut config = (*state.config).clone();
    config.workspace_root = temp.path().into();
    state.config = Arc::new(config);
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    headers.insert(
        "x-df-runner-isolation",
        HeaderValue::from_static("principal"),
    );
    headers.insert(
        "x-df-external-user-id",
        HeaderValue::from_static("person-a"),
    );
    headers.insert(
        "x-df-workspace-key",
        HeaderValue::from_static("not-resource"),
    );
    headers.insert("x-df-lease-owner", HeaderValue::from_static("person-a"));
    assert!(
        crate::auth::identity::mcp_actor_from_bearer(&state, &headers, false)
            .await
            .is_err()
    );
    assert!(
        queries::find_workspace_binding_any(&pool, 817301, "person-a", "not-resource")
            .await?
            .is_none()
    );
    headers.insert(
        "x-df-workspace-key",
        HeaderValue::from_static("artifact:one"),
    );
    headers.insert("x-df-lease-owner", HeaderValue::from_static("person-b"));
    assert!(
        crate::auth::identity::mcp_actor_from_bearer(&state, &headers, false)
            .await
            .is_err()
    );
    assert!(
        queries::find_workspace_binding_by_resource(&pool, 817301, "artifact", "one")
            .await?
            .is_none()
    );
    headers.insert("x-df-lease-owner", HeaderValue::from_static("person-a"));
    headers.append(
        "x-df-external-user-id",
        HeaderValue::from_static("person-a"),
    );
    assert!(
        crate::auth::identity::mcp_actor_from_bearer(&state, &headers, false)
            .await
            .is_err()
    );
    assert!(
        queries::find_workspace_binding_by_resource(&pool, 817301, "artifact", "one")
            .await?
            .is_none()
    );
    headers.insert(
        "x-df-external-user-id",
        HeaderValue::from_static("person-a"),
    );
    let carried = crate::auth::identity::mcp_actor_from_bearer(&state, &headers, false)
        .await
        .expect("valid auth")
        .unwrap();
    let actor = actor_from_mcp_actor(&state, carried.clone()).expect("valid actor");
    let expected = application_principal_owner(
        817301,
        actor.workspace_binding_id,
        "person-a",
        "artifact:one",
    )
    .unwrap();
    assert_eq!(actor.runner_owner(), expected);
    assert!(actor.ensure_write_access().is_err());
    let mut forged = carried;
    if let McpActor::ApplicationSubject {
        isolated_runner_owner,
        ..
    } = &mut forged
    {
        *isolated_runner_owner = Some(
            application_principal_owner(
                817301,
                actor.workspace_binding_id,
                "person-b",
                "artifact:one",
            )
            .unwrap(),
        );
    } else {
        panic!("application subject expected");
    }
    assert!(actor_from_mcp_actor(&state, forged).is_err());
    headers.insert(
        "x-df-external-user-id",
        HeaderValue::from_static("person-b"),
    );
    headers.insert("x-df-lease-owner", HeaderValue::from_static("person-b"));
    let other = crate::auth::identity::current_actor_from_bearer(&state, &headers, false)
        .await
        .expect("other auth")
        .unwrap();
    assert_eq!(other.workspace_binding_id, actor.workspace_binding_id);
    assert_ne!(
        other.runner_owner().container_name(),
        actor.runner_owner().container_name()
    );
    assert_eq!(
        other.policy.allows(crate::policy::WORKSPACE_SHELL),
        actor.policy.allows(crate::policy::WORKSPACE_SHELL)
    );
    headers.insert(
        "x-df-external-user-id",
        HeaderValue::from_static("person-a"),
    );
    headers.insert("x-df-lease-owner", HeaderValue::from_static("person-a"));
    headers.remove("x-df-runner-isolation");
    let legacy = crate::auth::identity::current_actor_from_bearer(&state, &headers, false)
        .await
        .expect("legacy auth")
        .unwrap();
    assert_eq!(
        legacy.runner_owner(),
        runner_protocol::RunnerOwner::WorkspaceBinding {
            workspace_binding_id: actor.workspace_binding_id,
        }
    );
    headers.insert(
        "x-df-runner-isolation",
        HeaderValue::from_static("principal"),
    );
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {user_token}"))?,
    );
    assert!(
        crate::auth::identity::mcp_actor_from_bearer(&state, &headers, false)
            .await
            .is_err()
    );
    pool.close().await;
    Ok(())
}

#[test]
fn mode_and_claims_are_unique_exact_and_bounded() {
    let mut headers = HeaderMap::new();
    assert!(!requested(&headers).unwrap());
    for invalid in [
        "",
        "Principal",
        " principal",
        "principal ",
        "other",
        &"x".repeat(1025),
    ] {
        headers.insert(
            RUNNER_ISOLATION_HEADER,
            HeaderValue::from_str(invalid).unwrap(),
        );
        assert!(requested(&headers).is_err());
    }
    headers.insert(
        RUNNER_ISOLATION_HEADER,
        HeaderValue::from_static("principal"),
    );
    assert!(requested(&headers).unwrap());
    headers.append(
        RUNNER_ISOLATION_HEADER,
        HeaderValue::from_static("principal"),
    );
    assert!(requested(&headers).is_err());
    headers.insert("x-claim", HeaderValue::from_static("value"));
    assert_eq!(required_claim(&headers, "x-claim").unwrap(), "value");
    headers.append("x-claim", HeaderValue::from_static("value"));
    assert!(required_claim(&headers, "x-claim").is_err());
}
