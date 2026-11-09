//! Internal application upstream adapter tests (issue 1180 P29A).
//!
//! The unit cases keep the route contract cheap. The ignored case is the only
//! DB-backed adapter test; it targets the dedicated nonproduction issue
//! database through `P29A_TEST_DATABASE_URL` and removes only its own rows.

use super::types::{
    CreateInternalWorkspaceGitUpstreamRequest, DeleteInternalWorkspaceGitUpstreamQuery,
    UpdateInternalWorkspaceGitUpstreamRequest,
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

    sqlx::query("DELETE FROM applications WHERE application_id = $1")
        .bind(app.application_id)
        .execute(&pool)
        .await?;
    let _ = std::fs::remove_dir_all(&root);
    pool.close().await;
    Ok(())
}
