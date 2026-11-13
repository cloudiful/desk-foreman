use super::*;
use crate::credential_cipher::{CredentialProtector, MasterKey};

fn protector() -> CredentialProtector {
    CredentialProtector::new(MasterKey::from_bytes(&[7u8; 32]).expect("master key"))
}

fn current(username: Option<&str>, ciphertext: Option<Vec<u8>>) -> CurrentUpstream {
    CurrentUpstream {
        remote_alias: "origin".to_string(),
        target_url: "https://git.example.com/team/repo.git".to_string(),
        credential_username: username.map(str::to_string),
        credential_ciphertext: ciphertext,
    }
}

fn update_with<'a>(
    credential: Option<(&'a str, &'a str)>,
    clear_credential: bool,
) -> WorkspaceGitUpstreamUpdate<'a> {
    WorkspaceGitUpstreamUpdate {
        remote_alias: None,
        target_url: None,
        credential,
        clear_credential,
    }
}

#[test]
fn bounded_page_clamps_limit_and_offset() {
    assert_eq!(bounded_page(None, None), (DEFAULT_LIMIT, 0));
    assert_eq!(bounded_page(Some(0), Some(-5)), (1, 0));
    assert_eq!(bounded_page(Some(10_000), Some(3)), (MAX_LIMIT, 3));
    assert_eq!(bounded_page(Some(25), Some(40)), (25, 40));
}

#[test]
fn identical_credential_is_a_no_op() {
    let protector = protector();
    let frame = protector.seal(918001, 42, "super-secret").expect("seal");
    let row = current(Some("git-user"), Some(frame));

    let action = resolve_credential_action(
        &row,
        Some(&protector),
        918001,
        42,
        &update_with(Some(("git-user", "super-secret")), false),
    )
    .expect("resolve");

    assert!(matches!(action, CredentialAction::Keep));
}

#[test]
fn changed_credential_replaces_the_stored_frame() {
    let protector = protector();
    let frame = protector.seal(918001, 42, "super-secret").expect("seal");
    let row = current(Some("git-user"), Some(frame));

    let action = resolve_credential_action(
        &row,
        Some(&protector),
        918001,
        42,
        &update_with(Some(("git-user", "rotated-secret")), false),
    )
    .expect("resolve");
    assert!(matches!(
        action,
        CredentialAction::Replace("git-user", "rotated-secret")
    ));

    let action = resolve_credential_action(
        &row,
        Some(&protector),
        918001,
        42,
        &update_with(Some(("other-user", "super-secret")), false),
    )
    .expect("resolve");
    assert!(matches!(
        action,
        CredentialAction::Replace("other-user", "super-secret")
    ));
}

#[test]
fn explicit_clear_and_metadata_update_keep_the_stored_credential() {
    let protector = protector();
    let frame = protector.seal(918001, 42, "super-secret").expect("seal");
    let row = current(Some("git-user"), Some(frame.clone()));

    let action = resolve_credential_action(&row, None, 918001, 42, &update_with(None, true))
        .expect("resolve");
    assert!(matches!(action, CredentialAction::Clear));

    let action = resolve_credential_action(&row, None, 918001, 42, &update_with(None, false))
        .expect("resolve");
    assert!(matches!(action, CredentialAction::Keep));

    let anonymous = current(None, None);
    let action = resolve_credential_action(&anonymous, None, 918001, 42, &update_with(None, true))
        .expect("resolve");
    assert!(matches!(action, CredentialAction::Keep));
}

#[test]
fn a_credential_write_without_a_key_fails_closed() {
    let row = current(Some("git-user"), None);

    let error = resolve_credential_action(
        &row,
        None,
        918001,
        42,
        &update_with(Some(("git-user", "super-secret")), false),
    )
    .expect_err("a credential write needs a key");

    assert!(matches!(error, GitUpstreamError::CredentialKeyUnavailable));
}

#[test]
fn an_unreadable_credential_fails_closed() {
    let protector = protector();
    let frame = protector.seal(918001, 42, "super-secret").expect("seal");
    let row = current(Some("git-user"), Some(frame));

    let error = resolve_credential_action(
        &row,
        Some(&protector),
        918001,
        99,
        &update_with(Some(("git-user", "super-secret")), false),
    )
    .expect_err("a frame of another row must not open");

    assert!(matches!(error, GitUpstreamError::CredentialUnreadable));
}

#[tokio::test]
#[ignore = "requires disposable P27 PostgreSQL (P27_TEST_DATABASE_URL)"]
async fn upstream_lifecycle_stores_encrypted_credentials_and_bumps_generation() -> anyhow::Result<()>
{
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("P27_TEST_DATABASE_URL")?)
        .await?;
    let protector = protector();
    sqlx::raw_sql(include_str!(
        "../../sql/workspace_git_upstreams/test_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/workspace_git_upstreams/test_reset.sql"
    ))
    .execute(&pool)
    .await?;

    let created = create_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        NewWorkspaceGitUpstream {
            remote_alias: "origin",
            target_url: "https://git.example.com/team/repo.git",
            credential: Some(("git-user", "super-secret")),
        },
    )
    .await?;
    assert_eq!(created.capability_generation, 1);
    assert!(created.has_credential);
    assert!(created.enabled);

    let stored = sqlx::query_as::<_, (Option<String>, Option<Vec<u8>>)>(
        "SELECT credential_username, credential_ciphertext FROM workspace_git_upstreams \
         WHERE workspace_binding_id = $1 AND upstream_id = $2",
    )
    .bind(918001i64)
    .bind(created.upstream_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(stored.0.as_deref(), Some("git-user"));
    let frame = stored.1.expect("an encrypted credential is stored");
    assert_eq!(&frame[..4], b"C69S");
    assert!(!frame.windows(11).any(|window| window == b"super-secret"));

    let page = list_workspace_git_upstreams(&pool, 918001, Some(10), Some(0)).await?;
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].remote_alias, "origin");

    let duplicate = create_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        NewWorkspaceGitUpstream {
            remote_alias: "origin",
            target_url: "https://git.example.com/other.git",
            credential: None,
        },
    )
    .await;
    assert!(matches!(duplicate, Err(GitUpstreamError::DuplicateAlias)));

    let identical = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: Some("origin"),
            target_url: Some("https://git.example.com/team/repo.git"),
            credential: Some(("git-user", "super-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(identical.capability_generation, 1);
    assert_eq!(identical.updated_at, created.updated_at);
    assert!(identical.has_credential);

    let rotated_secret = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: Some(("git-user", "rotated-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(rotated_secret.capability_generation, 2);
    assert!(rotated_secret.has_credential);

    let rotated_secret_repeat = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: Some(("git-user", "rotated-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(rotated_secret_repeat.capability_generation, 2);
    assert_eq!(rotated_secret_repeat.updated_at, rotated_secret.updated_at);

    let rotated_user = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: Some(("git-user-2", "rotated-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(rotated_user.capability_generation, 3);

    let retargeted = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: Some("https://git.example.com/team/moved.git"),
            credential: None,
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(retargeted.capability_generation, 4);
    assert_eq!(
        retargeted.target_url,
        "https://git.example.com/team/moved.git"
    );

    let realiased = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: Some("origin-2"),
            target_url: None,
            credential: None,
            clear_credential: false,
        },
    )
    .await?;
    assert_eq!(realiased.capability_generation, 5);
    assert_eq!(realiased.remote_alias, "origin-2");

    // A reset keeps the username and drops the credential material, so the
    // row asks for a resupply instead of forwarding anonymously.
    sqlx::query(
        "UPDATE workspace_git_upstreams SET credential_ciphertext = NULL, \
         credential_updated_at = NULL WHERE workspace_binding_id = $1 AND upstream_id = $2",
    )
    .bind(918001i64)
    .bind(created.upstream_id)
    .execute(&pool)
    .await?;

    let gateway_row = resolve_gateway_upstream(&pool, 918001, "origin-2")
        .await?
        .expect("gateway row");
    assert!(gateway_row.requires_credential_resupply());
    assert_eq!(
        gateway_row.credential_username.as_deref(),
        Some("git-user-2")
    );

    let resupplied = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: Some(("git-user-2", "resupplied-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert!(resupplied.has_credential);
    assert_eq!(resupplied.capability_generation, 6);

    let stored = sqlx::query_as::<_, (Option<String>, Option<Vec<u8>>)>(
        "SELECT credential_username, credential_ciphertext FROM workspace_git_upstreams \
         WHERE workspace_binding_id = $1 AND upstream_id = $2",
    )
    .bind(918001i64)
    .bind(created.upstream_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(stored.0.as_deref(), Some("git-user-2"));
    let frame = stored.1.expect("an encrypted credential is stored");
    assert_eq!(&frame[..4], b"C69S");
    assert!(
        !frame
            .windows(15)
            .any(|window| window == b"resupplied-secret")
    );

    let cleared = update_workspace_git_upstream(
        &pool,
        None,
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: None,
            clear_credential: true,
        },
    )
    .await?;
    assert!(!cleared.has_credential);
    assert_eq!(cleared.capability_generation, 7);

    let cleared_repeat = update_workspace_git_upstream(
        &pool,
        None,
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: None,
            clear_credential: true,
        },
    )
    .await?;
    assert!(!cleared_repeat.has_credential);
    assert_eq!(cleared_repeat.capability_generation, 7);
    assert_eq!(cleared_repeat.updated_at, cleared.updated_at);

    let cross_binding = update_workspace_git_upstream(
        &pool,
        Some(&protector),
        918002,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: Some("moved"),
            target_url: None,
            credential: None,
            clear_credential: false,
        },
    )
    .await;
    assert!(matches!(cross_binding, Err(GitUpstreamError::NotFound)));

    let archived = create_workspace_git_upstream(
        &pool,
        Some(&protector),
        918003,
        NewWorkspaceGitUpstream {
            remote_alias: "origin",
            target_url: "https://git.example.com/team/repo.git",
            credential: None,
        },
    )
    .await;
    assert!(matches!(archived, Err(GitUpstreamError::BindingInactive)));

    delete_workspace_git_upstream(&pool, 918001, created.upstream_id).await?;
    assert!(matches!(
        delete_workspace_git_upstream(&pool, 918001, created.upstream_id).await,
        Err(GitUpstreamError::NotFound)
    ));

    sqlx::raw_sql(include_str!(
        "../../sql/workspace_git_upstreams/test_cleanup.sql"
    ))
    .execute(&pool)
    .await?;
    Ok(())
}
