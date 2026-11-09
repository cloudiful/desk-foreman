//! Internal application upstream adapter tests (issue 1180 P29A).
//!
//! The unit cases keep the route contract cheap. The ignored case is the only
//! DB-backed adapter test; it targets the dedicated nonproduction issue
//! database through `P29A_TEST_DATABASE_URL` and removes only its own rows.

use super::types::{
    CreateInternalWorkspaceGitUpstreamRequest, DeleteInternalWorkspaceGitUpstreamQuery,
    UpdateInternalWorkspaceGitUpstreamRequest, WorkspaceGitCapabilityRequest,
};
use crate::api::admin::workspace_git_upstreams::types::{
    CreateWorkspaceGitUpstreamRequest, UpdateWorkspaceGitUpstreamRequest,
};
use validator::Validate;

fn create_request(lease_owner: &str) -> CreateInternalWorkspaceGitUpstreamRequest {
    CreateInternalWorkspaceGitUpstreamRequest {
        lease_owner: lease_owner.to_string(),
        upstream: CreateWorkspaceGitUpstreamRequest {
            remote_alias: "origin".to_string(),
            target_url: "https://git.example.com/team/repo.git".to_string(),
            credential_username: None,
            credential_secret: None,
        },
    }
}

#[test]
fn create_and_update_require_a_bounded_lease_owner_and_valid_upstream() {
    assert!(create_request("lease-owner").validate().is_ok());
    assert!(create_request("").validate().is_err());
    assert!(create_request("bad\nowner").validate().is_err());
    assert!(create_request(&"a".repeat(257)).validate().is_err());

    let mut bad_url = create_request("lease-owner");
    bad_url.upstream.target_url = "ssh://git.example.com/repo.git".to_string();
    assert!(bad_url.validate().is_err());

    let update = UpdateInternalWorkspaceGitUpstreamRequest {
        lease_owner: "lease-owner".to_string(),
        upstream: UpdateWorkspaceGitUpstreamRequest {
            remote_alias: None,
            target_url: None,
            credential_username: Some("user".to_string()),
            credential_secret: None,
            clear_credential: false,
        },
    };
    assert!(update.validate().is_err());

    let clear = UpdateInternalWorkspaceGitUpstreamRequest {
        lease_owner: "lease-owner".to_string(),
        upstream: UpdateWorkspaceGitUpstreamRequest {
            remote_alias: None,
            target_url: None,
            credential_username: None,
            credential_secret: None,
            clear_credential: true,
        },
    };
    assert!(clear.validate().is_ok());
}

#[test]
fn delete_query_requires_a_bounded_lease_owner() {
    let query = |value: &str| DeleteInternalWorkspaceGitUpstreamQuery {
        lease_owner: value.to_string(),
    };
    assert!(query("lease-owner").validate().is_ok());
    assert!(query("").validate().is_err());
    assert!(query("bad\rowner").validate().is_err());
}

#[test]
fn delete_query_declares_lease_owner_as_a_query_parameter() {
    use utoipa::{IntoParams, openapi::path::ParameterIn};

    // The explicit `parameter_in = Query` wins even when the endpoint provider
    // suggests a different location, which is what keeps the exported
    // DELETE operation from advertising `lease_owner` as a path parameter.
    let params = DeleteInternalWorkspaceGitUpstreamQuery::into_params(|| Some(ParameterIn::Path));
    let lease_owner = params
        .iter()
        .find(|param| param.name == "lease_owner")
        .expect("lease_owner parameter is declared");
    assert!(matches!(lease_owner.parameter_in, ParameterIn::Query));
}

#[tokio::test]
#[ignore = "requires the dedicated issue PostgreSQL (P29A_TEST_DATABASE_URL)"]
async fn application_authority_is_validated_under_the_binding_row_lock() -> anyhow::Result<()> {
    use crate::{
        credential_cipher::UpstreamCredentialKey,
        db::{
            queries,
            types::CreateApplicationRequest,
            workspace_git_upstreams::{
                MutationAuthority, NewWorkspaceGitUpstream, WorkspaceGitUpstreamUpdate,
                create_workspace_git_upstream_for_application,
                delete_workspace_git_upstream_for_application, list_workspace_git_upstreams,
                update_workspace_git_upstream_for_application,
            },
        },
    };
    use sqlx::postgres::PgPoolOptions;

    let url = std::env::var("P29A_TEST_DATABASE_URL")
        .expect("P29A_TEST_DATABASE_URL must point at the dedicated issue database");
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await?;

    let app = queries::create_application(
        &pool,
        &CreateApplicationRequest {
            name: format!("p29a-adapter-{}", uuid::Uuid::new_v4()),
            workspace_template: None,
            default_shell: None,
            default_scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await?;
    let root = std::env::temp_dir().join(format!("p29a-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    let binding = queries::create_workspace_binding(
        &pool,
        app.application_id,
        "subject-one",
        "default",
        root.to_str().unwrap(),
        None,
        None,
    )
    .await?;
    let other_binding = queries::create_workspace_binding(
        &pool,
        app.application_id,
        "subject-two",
        "default",
        root.to_str().unwrap(),
        None,
        None,
    )
    .await?;

    let key = UpstreamCredentialKey::parse(&"11".repeat(32))?.expect("key");

    let created = create_workspace_git_upstream_for_application(
        &pool,
        Some(&key),
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-one",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        NewWorkspaceGitUpstream {
            remote_alias: "origin",
            target_url: "https://git.example.com/team/repo.git",
            credential: Some(("git-user", "super-secret")),
        },
    )
    .await?;
    assert_eq!(created.capability_generation, 1);
    assert!(created.has_credential);
    assert!(!format!("{created:?}").contains("super-secret"));

    let page = list_workspace_git_upstreams(&pool, binding.workspace_binding_id, Some(10), Some(0))
        .await?;
    assert_eq!(page.total, 1);

    let wrong_app = create_workspace_git_upstream_for_application(
        &pool,
        Some(&key),
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id + 1,
            external_user_id: "subject-one",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        NewWorkspaceGitUpstream {
            remote_alias: "foreign",
            target_url: "https://git.example.com/foreign.git",
            credential: None,
        },
    )
    .await;
    assert!(matches!(
        wrong_app,
        Err(crate::db::workspace_git_upstreams::GitUpstreamError::Forbidden)
    ));

    let wrong_subject = create_workspace_git_upstream_for_application(
        &pool,
        Some(&key),
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-two",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        NewWorkspaceGitUpstream {
            remote_alias: "other-subject",
            target_url: "https://git.example.com/other.git",
            credential: None,
        },
    )
    .await;
    assert!(matches!(
        wrong_subject,
        Err(crate::db::workspace_git_upstreams::GitUpstreamError::Forbidden)
    ));

    let updated = update_workspace_git_upstream_for_application(
        &pool,
        Some(&key),
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-one",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: Some("origin-2"),
            target_url: None,
            credential: None,
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(updated.capability_generation, 2);

    let identical = update_workspace_git_upstream_for_application(
        &pool,
        Some(&key),
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-one",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: Some("origin-2"),
            target_url: Some("https://git.example.com/team/repo.git"),
            credential: Some(("git-user", "super-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(identical.capability_generation, 2);
    assert_eq!(identical.updated_at, updated.updated_at);

    let cross_binding = update_workspace_git_upstream_for_application(
        &pool,
        Some(&key),
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-two",
            lease_owner: "",
        },
        other_binding.workspace_binding_id,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: Some("moved"),
            target_url: None,
            credential: None,
            clear_credential: false,
        },
    )
    .await;
    assert!(matches!(
        cross_binding,
        Err(crate::db::workspace_git_upstreams::GitUpstreamError::NotFound)
    ));

    let missing_key = create_workspace_git_upstream_for_application(
        &pool,
        None,
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-one",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        NewWorkspaceGitUpstream {
            remote_alias: "needs-key",
            target_url: "https://git.example.com/key.git",
            credential: Some(("user", "secret")),
        },
    )
    .await;
    assert!(matches!(
        missing_key,
        Err(crate::db::workspace_git_upstreams::GitUpstreamError::CredentialKeyMissing)
    ));

    delete_workspace_git_upstream_for_application(
        &pool,
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-one",
            lease_owner: "",
        },
        binding.workspace_binding_id,
        created.upstream_id,
    )
    .await?;
    assert!(matches!(
        delete_workspace_git_upstream_for_application(
            &pool,
            &MutationAuthority::ApplicationSubject {
                application_id: app.application_id,
                external_user_id: "subject-one",
                lease_owner: "",
            },
            binding.workspace_binding_id,
            created.upstream_id,
        )
        .await,
        Err(crate::db::workspace_git_upstreams::GitUpstreamError::NotFound)
    ));

    // Lease authority is re-read under the binding row lock: a concurrent
    // handover cannot be bypassed by a pre-lock check.
    let resource = queries::create_workspace_binding(
        &pool,
        app.application_id,
        "subject-one",
        "issue:42",
        root.to_str().unwrap(),
        Some("issue"),
        Some("42"),
    )
    .await?;
    let lease_id = resource.workspace_binding_id;
    let mut holder = pool.begin().await?;
    sqlx::query(
        "UPDATE workspace_bindings SET write_lease_owner = 'owner-a', \
         write_lease_expires_at = NOW() + interval '10 minutes' \
         WHERE workspace_binding_id = $1",
    )
    .bind(lease_id)
    .execute(&mut *holder)
    .await?;
    sqlx::query("SELECT 1 FROM workspace_bindings WHERE workspace_binding_id = $1 FOR UPDATE")
        .bind(lease_id)
        .execute(&mut *holder)
        .await?;

    let pool_for_task = pool.clone();
    let app_id = app.application_id;
    let blocked = tokio::spawn(async move {
        create_workspace_git_upstream_for_application(
            &pool_for_task,
            None,
            &MutationAuthority::ApplicationSubject {
                application_id: app_id,
                external_user_id: "subject-one",
                lease_owner: "owner-a",
            },
            lease_id,
            NewWorkspaceGitUpstream {
                remote_alias: "race",
                target_url: "https://git.example.com/race.git",
                credential: None,
            },
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    sqlx::query(
        "UPDATE workspace_bindings SET write_lease_owner = 'owner-b' \
         WHERE workspace_binding_id = $1",
    )
    .bind(lease_id)
    .execute(&mut *holder)
    .await?;
    holder.commit().await?;

    assert!(matches!(
        blocked.await?,
        Err(crate::db::workspace_git_upstreams::GitUpstreamError::LeaseConflict)
    ));

    let current_lease = create_workspace_git_upstream_for_application(
        &pool,
        None,
        &MutationAuthority::ApplicationSubject {
            application_id: app.application_id,
            external_user_id: "subject-one",
            lease_owner: "owner-b",
        },
        lease_id,
        NewWorkspaceGitUpstream {
            remote_alias: "lease-ok",
            target_url: "https://git.example.com/ok.git",
            credential: None,
        },
    )
    .await?;
    assert_eq!(current_lease.capability_generation, 1);

    // Resource principals keep the stored `__resource__` marker internally but
    // authenticate as the real external user. The caller presents the isolated
    // scope headers and the live lease; the resolved authority must name this
    // exact binding and keep the marker out of the client identity.
    sqlx::query(
        "UPDATE workspace_bindings SET external_user_id = '__resource__', \
         write_lease_owner = 'person-a', \
         write_lease_expires_at = NOW() + interval '10 minutes' \
         WHERE workspace_binding_id = $1",
    )
    .bind(lease_id)
    .execute(&pool)
    .await?;

    let (token, _) = queries::create_application_token(
        &pool,
        &crate::db::types::CreateApplicationTokenRequest {
            application_id: app.application_id,
            token_name: "p36-resource-authority".to_string(),
            expires_at: None,
            scopes: Some(vec![
                crate::policy::WORKSPACE_PATCH.to_string(),
                crate::policy::WORKSPACE_SHELL.to_string(),
            ]),
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await?;

    let mut state =
        crate::lease_test_support::fake_state(crate::lease_test_support::FakeRunnerService::new());
    state.db = pool.clone();
    {
        let config = std::sync::Arc::make_mut(&mut state.config);
        config.upstream_credential_key =
            Some(UpstreamCredentialKey::parse(&"11".repeat(32))?.expect("key"));
        config.git_gateway = crate::config::GitGatewayConfig {
            public_base_url: Some("http://gateway.test".to_string()),
            ..Default::default()
        };
    }

    use axum::http::{HeaderMap, header::AUTHORIZATION};
    let isolated_headers = || -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {token}").parse().expect("authorization"),
        );
        headers.insert(
            "X-DF-Runner-Isolation",
            "principal".parse().expect("isolation"),
        );
        headers.insert(
            "X-DF-External-User-Id",
            "person-a".parse().expect("external user"),
        );
        headers.insert(
            "X-DF-Workspace-Key",
            "issue:42".parse().expect("workspace key"),
        );
        headers.insert("X-DF-Lease-Owner", "person-a".parse().expect("lease owner"));
        headers
    };
    let headers = isolated_headers();

    let authority = super::resolve_git_binding(&state, &headers, lease_id, true)
        .await
        .expect("resource authority");
    assert_eq!(authority.identity_subject, "person-a");
    assert_eq!(authority.authority_subject, "__resource__");
    assert_eq!(authority.lease_owner.as_deref(), Some("person-a"));
    assert_eq!(authority.binding.workspace_binding_id, lease_id);
    assert!(matches!(
        super::effective_lease(&authority, Some("person-b")),
        Err(crate::error::AppError::Forbidden(_))
    ));

    let capability = super::issue_git_capability(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(lease_id),
        crate::api::validation::ValidatedJson(WorkspaceGitCapabilityRequest {
            external_user_id: "person-a".to_string(),
            lease_owner: Some("person-a".to_string()),
            principal_id: None,
        }),
    )
    .await
    .expect("resource capability");
    let signing_key = crate::git_gateway::signing_key(&state.config).expect("signing key");
    let claims = crate::git_gateway::capability::verify(
        &signing_key,
        &capability.0.capability,
        chrono::Utc::now().timestamp(),
    )
    .expect("capability verifies");
    assert_eq!(claims.external_user_id, "__resource__");
    assert_eq!(claims.lease_owner, "person-a");
    assert_eq!(
        claims.principal_id,
        crate::git_gateway::capability::principal_id(app.application_id, "person-a", "issue:42")
    );
    assert!(claims.write);

    let mismatched_lease = super::issue_git_capability(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(lease_id),
        crate::api::validation::ValidatedJson(WorkspaceGitCapabilityRequest {
            external_user_id: "person-a".to_string(),
            lease_owner: Some("person-b".to_string()),
            principal_id: None,
        }),
    )
    .await;
    assert!(matches!(
        mismatched_lease,
        Err(crate::error::AppError::Forbidden(_))
    ));

    let mut missing_isolation = isolated_headers();
    missing_isolation.remove("X-DF-Runner-Isolation");
    assert!(matches!(
        super::resolve_git_binding(&state, &missing_isolation, lease_id, false).await,
        Err(crate::error::AppError::Forbidden(_))
    ));

    assert!(matches!(
        super::resolve_git_binding(&state, &headers, binding.workspace_binding_id, false).await,
        Err(crate::error::AppError::Forbidden(_))
    ));

    let list_params = || crate::db::types::ListWorkspaceGitUpstreamsParams {
        limit: None,
        offset: None,
    };
    let live_list = super::list_application_git_upstreams(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(lease_id),
        crate::api::validation::ValidatedQuery(list_params()),
    )
    .await
    .expect("live resource list");
    assert!(!live_list.0.items.is_empty());

    // A matching principal/lease header pair that no longer holds the binding
    // lease must not disclose another owner's upstream metadata.
    let mut other_holder = isolated_headers();
    other_holder.insert(
        "X-DF-External-User-Id",
        "person-b".parse().expect("external user"),
    );
    other_holder.insert("X-DF-Lease-Owner", "person-b".parse().expect("lease owner"));
    assert!(matches!(
        super::list_application_git_upstreams(
            axum::extract::State(state.clone()),
            other_holder,
            axum::extract::Path(lease_id),
            crate::api::validation::ValidatedQuery(list_params()),
        )
        .await,
        Err(crate::error::AppError::Forbidden(_))
    ));

    // An expired lease held by the named owner is refused too.
    sqlx::query(
        "UPDATE workspace_bindings SET write_lease_expires_at = NOW() - interval '1 second' \
         WHERE workspace_binding_id = $1",
    )
    .bind(lease_id)
    .execute(&pool)
    .await?;
    assert!(matches!(
        super::list_application_git_upstreams(
            axum::extract::State(state.clone()),
            headers.clone(),
            axum::extract::Path(lease_id),
            crate::api::validation::ValidatedQuery(list_params()),
        )
        .await,
        Err(crate::error::AppError::Forbidden(_))
    ));
    sqlx::query(
        "UPDATE workspace_bindings SET write_lease_expires_at = NOW() + interval '10 minutes' \
         WHERE workspace_binding_id = $1",
    )
    .bind(lease_id)
    .execute(&pool)
    .await?;

    // External user and lease owner must agree in the isolated identity.
    let mut mismatched_principal = isolated_headers();
    mismatched_principal.insert(
        "X-DF-External-User-Id",
        "person-b".parse().expect("external user"),
    );
    assert!(matches!(
        super::list_application_git_upstreams(
            axum::extract::State(state.clone()),
            mismatched_principal,
            axum::extract::Path(lease_id),
            crate::api::validation::ValidatedQuery(list_params()),
        )
        .await,
        Err(crate::error::AppError::Unauthorized(_))
    ));

    // The isolated scope must name the exact addressed binding.
    let other_resource = queries::create_workspace_binding(
        &pool,
        app.application_id,
        "__resource__",
        "issue:99",
        root.to_str().unwrap(),
        Some("issue"),
        Some("99"),
    )
    .await?;
    assert!(matches!(
        super::list_application_git_upstreams(
            axum::extract::State(state.clone()),
            headers.clone(),
            axum::extract::Path(other_resource.workspace_binding_id),
            crate::api::validation::ValidatedQuery(list_params()),
        )
        .await,
        Err(crate::error::AppError::Forbidden(_))
    ));

    // Personal read semantics are unchanged: no isolation, workspace-key or
    // lease-owner header is needed.
    let mut personal_headers = HeaderMap::new();
    personal_headers.insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().expect("authorization"),
    );
    personal_headers.insert(
        "X-DF-External-User-Id",
        "subject-one".parse().expect("external user"),
    );
    let personal_list = super::list_application_git_upstreams(
        axum::extract::State(state.clone()),
        personal_headers,
        axum::extract::Path(binding.workspace_binding_id),
        crate::api::validation::ValidatedQuery(list_params()),
    )
    .await
    .expect("personal list");
    assert!(personal_list.0.items.is_empty());

    sqlx::query("DELETE FROM applications WHERE application_id = $1")
        .bind(app.application_id)
        .execute(&pool)
        .await?;
    let _ = std::fs::remove_dir_all(&root);
    pool.close().await;
    Ok(())
}
