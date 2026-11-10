use super::*;

#[test]
fn bounded_page_clamps_limit_and_offset() {
    assert_eq!(bounded_page(None, None), (DEFAULT_LIMIT, 0));
    assert_eq!(bounded_page(Some(0), Some(-5)), (1, 0));
    assert_eq!(bounded_page(Some(10_000), Some(3)), (MAX_LIMIT, 3));
    assert_eq!(bounded_page(Some(25), Some(40)), (25, 40));
}

#[tokio::test]
#[ignore = "requires disposable P27 PostgreSQL (P27_TEST_DATABASE_URL)"]
async fn upstream_lifecycle_stores_plaintext_and_bumps_generation() -> anyhow::Result<()> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("P27_TEST_DATABASE_URL")?)
        .await?;
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

    let page = list_workspace_git_upstreams(&pool, 918001, Some(10), Some(0)).await?;
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].remote_alias, "origin");

    let duplicate = create_workspace_git_upstream(
        &pool,
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

    // Simulate an upgrade from the old encrypted format: only the legacy
    // ciphertext remains and its bytes are opaque, so any decrypt attempt would
    // fail. Metadata-only edits keep it and still report a credential.
    sqlx::query(
        "UPDATE workspace_git_upstreams SET credential_secret = NULL, credential_ciphertext = $1 \
         WHERE workspace_binding_id = $2 AND upstream_id = $3",
    )
    .bind(vec![9u8, 9, 9, 9])
    .bind(918001i64)
    .bind(created.upstream_id)
    .execute(&pool)
    .await?;

    let legacy_kept = update_workspace_git_upstream(
        &pool,
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: Some("https://git.example.com/team/legacy.git"),
            credential: None,
            clear_credential: false,
        },
    )
    .await?;
    assert!(legacy_kept.has_credential);
    assert_eq!(legacy_kept.capability_generation, 6);

    // A supplied credential replaces the legacy row without decrypting it.
    let legacy_replaced = update_workspace_git_upstream(
        &pool,
        918001,
        created.upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: None,
            credential: Some(("git-user", "new-secret")),
            clear_credential: false,
        },
    )
    .await?;
    assert!(legacy_replaced.has_credential);
    assert_eq!(legacy_replaced.capability_generation, 7);
    let stored = sqlx::query_as::<_, (Option<String>, Option<Vec<u8>>)>(
        "SELECT credential_secret, credential_ciphertext FROM workspace_git_upstreams \
         WHERE workspace_binding_id = $1 AND upstream_id = $2",
    )
    .bind(918001i64)
    .bind(created.upstream_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(stored.0.as_deref(), Some("new-secret"));
    assert!(stored.1.is_none());

    let cleared = update_workspace_git_upstream(
        &pool,
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
    assert_eq!(cleared.capability_generation, 8);

    let cleared_repeat = update_workspace_git_upstream(
        &pool,
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
    assert_eq!(cleared_repeat.capability_generation, 8);
    assert_eq!(cleared_repeat.updated_at, cleared.updated_at);

    let disabled =
        set_workspace_git_upstream_enabled(&pool, 918001, created.upstream_id, false).await?;
    assert!(!disabled.enabled);
    assert_eq!(disabled.capability_generation, 9);

    let cross_binding = update_workspace_git_upstream(
        &pool,
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
