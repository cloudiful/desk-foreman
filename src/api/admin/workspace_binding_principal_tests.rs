use super::*;
use crate::api::admin::lease_helpers::cancel_binding_sessions_best_effort;
use crate::db::{
    principal_admission,
    workspace_takeover::{TakeoverOutcome, acquire_workspace_write_lease_takeover},
};
use runner_protocol::workspace_principal::application_principal_owner;
use std::sync::{Arc, atomic::Ordering};

#[path = "lease_test_support.rs"]
mod support;

#[tokio::test]
#[ignore = "requires newly initialized disposable P4k3 PostgreSQL"]
async fn ordinary_lease_api_runs_durable_cleanup() -> anyhow::Result<()> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("P4K3_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_lease_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_lease_expire.sql"
    ))
    .bind(817402_i64)
    .execute(&pool)
    .await?;
    let owner = application_principal_owner(817401, 817402, "old", "workspace").unwrap();
    principal_admission::admit(&pool, 17, &owner, "container").await?;
    let runner = support::FakeRunnerService::new();
    runner.fail_principal.store(true, Ordering::SeqCst);
    let mut state = support::fake_state(runner.clone());
    state.db = pool.clone();
    assert!(matches!(
        acquire_binding_lease(&state, 817402, "new", 60).await,
        Err(AppError::ServiceUnavailable(_))
    ));
    assert_eq!(
        crate::db::queries::find_workspace_binding_by_id(&pool, 817402)
            .await?
            .unwrap()
            .write_lease_owner
            .as_deref(),
        Some("old")
    );
    assert_eq!(
        principal_admission::admit(&pool, 17, &owner, "late").await?,
        principal_admission::AdmissionOutcome::Fenced
    );
    runner.fail_principal.store(false, Ordering::SeqCst);
    assert_eq!(
        acquire_binding_lease(&state, 817402, "new", 60)
            .await
            .expect("retry succeeds")
            .write_lease_owner
            .as_deref(),
        Some("new")
    );
    assert!(release_binding_lease(&state, 817402, "new").await.is_ok());
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4k2 PostgreSQL"]
async fn guarded_lifecycle_fails_closed_then_retries() -> anyhow::Result<()> {
    let url = std::env::var("P4K2_TEST_DATABASE_URL")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_lifecycle_seed.sql"
    ))
    .execute(&pool)
    .await?;
    let temp = tempfile::tempdir()?;
    for (id, user) in [
        (817302, "takeover"),
        (817303, "reset"),
        (817304, "archive"),
        (817305, "janitor"),
        (817306, "other"),
        (817307, "native"),
    ] {
        let root = temp.path().join(user);
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("keep"), "keep")?;
        sqlx::query(include_str!(
            "../../sql/principal_admission/test_lifecycle_binding.sql"
        ))
        .bind(id)
        .bind(user)
        .bind(root.to_str().unwrap())
        .execute(&pool)
        .await?;
        let owner = application_principal_owner(817301, id, user, "workspace").unwrap();
        principal_admission::admit(&pool, 17, &owner, "container").await?;
    }
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_lifecycle_archive.sql"
    ))
    .bind(817305_i64)
    .execute(&pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_lifecycle_native.sql"
    ))
    .execute(&pool)
    .await?;
    let runner = support::FakeRunnerService::new();
    runner.fail_principal.store(true, Ordering::SeqCst);
    let mut state = support::fake_state(runner.clone());
    state.db = pool.clone();
    let mut config = (*state.config).clone();
    config.workspace_root = temp.path().into();
    state.config = Arc::new(config);
    let jar = CookieJar::new().add(axum_extra::extract::cookie::Cookie::new(
        "desk_foreman_session",
        "00000000-0000-4000-8000-000000000301",
    ));
    assert!(matches!(
        reset_workspace_binding(State(state.clone()), jar.clone(), Path(817307)).await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        archive_workspace_binding(State(state.clone()), jar.clone(), Path(817307)).await,
        Err(AppError::Conflict(_))
    ));
    assert!(temp.path().join("native/keep").exists());
    assert!(
        principal_admission::list_fences_by_binding(&pool, 817307)
            .await?
            .is_empty()
    );
    let takeover = || {
        acquire_workspace_write_lease_takeover(
            &pool,
            817302,
            "new",
            600,
            "takeover",
            0,
            false,
            cancel_binding_sessions_best_effort(&state, 817302),
        )
    };
    let failed = tokio::time::timeout(std::time::Duration::from_secs(3), takeover()).await??;
    assert!(matches!(failed, TakeoverOutcome::QuiescenceFailed { .. }));
    assert_eq!(
        crate::db::queries::find_workspace_binding_by_id(&pool, 817302)
            .await?
            .unwrap()
            .write_lease_owner
            .as_deref(),
        Some("takeover")
    );
    assert!(matches!(
        reset_workspace_binding(State(state.clone()), jar.clone(), Path(817303)).await,
        Err(AppError::ServiceUnavailable(_))
    ));
    assert!(temp.path().join("reset/keep").exists());
    assert!(matches!(
        archive_workspace_binding(State(state.clone()), jar.clone(), Path(817304)).await,
        Err(AppError::ServiceUnavailable(_))
    ));
    assert_eq!(
        crate::db::queries::find_workspace_binding_by_id(&pool, 817304)
            .await?
            .unwrap()
            .lifecycle_state,
        "active"
    );
    crate::lifecycle::cleanup_archived_workspaces(&state).await?;
    assert!(temp.path().join("janitor/keep").exists());
    assert!(
        crate::db::queries::find_workspace_binding_by_id(&pool, 817305)
            .await?
            .is_some()
    );
    for (id, user) in [
        (817302, "takeover"),
        (817303, "reset"),
        (817304, "archive"),
        (817305, "janitor"),
    ] {
        let owner = application_principal_owner(817301, id, user, "workspace").unwrap();
        assert_eq!(
            principal_admission::admit(&pool, 17, &owner, "late").await?,
            principal_admission::AdmissionOutcome::Fenced
        );
        assert_eq!(
            principal_admission::list_active_by_owner(&pool, &owner)
                .await?
                .len(),
            1
        );
    }
    let untouched = application_principal_owner(817301, 817306, "other", "workspace").unwrap();
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &untouched)
            .await?
            .len(),
        1
    );
    runner.fail_principal.store(false, Ordering::SeqCst);
    runner.fail_cleanup.store(true, Ordering::SeqCst);
    assert!(matches!(
        reset_workspace_binding(State(state.clone()), jar.clone(), Path(817303)).await,
        Err(AppError::ServiceUnavailable(_))
    ));
    assert!(temp.path().join("reset/keep").exists());
    runner.fail_cleanup.store(false, Ordering::SeqCst);
    assert!(matches!(takeover().await?, TakeoverOutcome::Success { .. }));
    let _ = reset_workspace_binding(State(state.clone()), jar.clone(), Path(817303))
        .await
        .expect("reset succeeds");
    assert!(!temp.path().join("reset/keep").exists());
    let _ = archive_workspace_binding(State(state.clone()), jar, Path(817304))
        .await
        .expect("archive succeeds");
    assert_eq!(
        crate::db::queries::find_workspace_binding_by_id(&pool, 817304)
            .await?
            .unwrap()
            .lifecycle_state,
        "archived"
    );
    crate::lifecycle::cleanup_archived_workspaces(&state).await?;
    assert!(!temp.path().join("janitor").exists());
    assert!(
        crate::db::queries::find_workspace_binding_by_id(&pool, 817305)
            .await?
            .is_none()
    );
    pool.close().await;
    Ok(())
}
