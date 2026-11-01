use super::*;
use axum::http::{HeaderMap, HeaderValue};

#[path = "../api/admin/lease_test_support.rs"]
mod support;

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
        HeaderValue::from_static("code_project:one"),
    );
    headers.insert("x-df-lease-owner", HeaderValue::from_static("person-b"));
    assert!(
        crate::auth::identity::mcp_actor_from_bearer(&state, &headers, false)
            .await
            .is_err()
    );
    assert!(
        queries::find_workspace_binding_by_resource(&pool, 817301, "code_project", "one")
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
        queries::find_workspace_binding_by_resource(&pool, 817301, "code_project", "one")
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
        "code_project:one",
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
                "code_project:one",
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
            workspace_binding_id: actor.workspace_binding_id
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
