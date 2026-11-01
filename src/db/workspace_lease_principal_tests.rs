use super::*;
use runner_protocol::workspace_principal::application_principal_owner;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
#[ignore = "requires newly initialized disposable P4k3 PostgreSQL"]
async fn ordinary_lease_cleanup_transactions() -> anyhow::Result<()> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("P4K3_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_lease_seed.sql"
    ))
    .execute(&pool)
    .await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = || {
        let calls = calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    };
    assert!(matches!(
        acquire_workspace_write_lease_with_cleanup(&pool, 817402, "old", 60, count()).await?,
        PrincipalLeaseOutcome::Updated(_)
    ));
    assert!(matches!(
        acquire_workspace_write_lease_with_cleanup(&pool, 817402, "foreign", 60, count()).await?,
        PrincipalLeaseOutcome::Conflict
    ));
    assert!(matches!(
        release_workspace_write_lease_with_cleanup(&pool, 817402, "foreign", count()).await?,
        PrincipalLeaseOutcome::Conflict
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        release_workspace_write_lease_with_cleanup(&pool, 817403, "free", count()).await?,
        PrincipalLeaseOutcome::Updated(_)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut native = pool.begin().await?;
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_lease_native.sql"
    ))
    .execute(&mut *native)
    .await?;
    let p = pool.clone();
    let c = calls.clone();
    let mut racing_native = tokio::spawn(async move {
        acquire_workspace_write_lease_with_cleanup(&p, 817403, "new", 60, async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut racing_native)
            .await
            .is_err()
    );
    native.commit().await?;
    assert!(matches!(
        racing_native.await??,
        PrincipalLeaseOutcome::Conflict
    ));
    assert!(matches!(
        release_workspace_write_lease_with_cleanup(&pool, 817403, "free", count()).await?,
        PrincipalLeaseOutcome::Conflict
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    sqlx::query(include_str!(
        "../sql/principal_admission/test_lease_expire.sql"
    ))
    .bind(817402_i64)
    .execute(&pool)
    .await?;
    let old = application_principal_owner(817401, 817402, "old", "workspace").unwrap();
    let failed = acquire_workspace_write_lease_with_cleanup(&pool, 817402, "new", 60, async {
        crate::db::principal_admission::fence(&pool, &old).await?;
        anyhow::bail!("offline")
    })
    .await?;
    assert!(matches!(
        failed,
        PrincipalLeaseOutcome::PrincipalCleanupFailed
    ));
    assert_eq!(
        crate::db::principal_admission::admit(&pool, 1, &old, "late").await?,
        crate::db::principal_admission::AdmissionOutcome::Fenced
    );
    let current = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_by_id.sql"
    ))
    .bind(817402_i64)
    .fetch_one(&pool)
    .await?;
    assert_eq!(current.write_lease_owner.as_deref(), Some("old"));
    let timed = acquire_workspace_write_lease_with_cleanup(
        &pool,
        817402,
        "new",
        60,
        std::future::pending(),
    )
    .await?;
    assert!(matches!(
        timed,
        PrincipalLeaseOutcome::PrincipalCleanupFailed
    ));
    let current = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_by_id.sql"
    ))
    .bind(817402_i64)
    .fetch_one(&pool)
    .await?;
    assert_eq!(current.write_lease_owner.as_deref(), Some("old"));
    let acquired = acquire_workspace_write_lease_with_cleanup(&pool, 817402, "new", 60, async {
        let row = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
            "../sql/find_workspace_binding_by_id.sql"
        ))
        .bind(817402_i64)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.write_lease_owner.as_deref(), Some("old"));
        Ok(())
    })
    .await?;
    assert!(matches!(acquired, PrincipalLeaseOutcome::Updated(_)));
    assert!(matches!(
        release_workspace_write_lease_with_cleanup(&pool, 817402, "new", count()).await?,
        PrincipalLeaseOutcome::Updated(_)
    ));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let p = pool.clone();
    let first = tokio::spawn(async move {
        acquire_workspace_write_lease_with_cleanup(&p, 817402, "first", 60, async move {
            let _ = entered_tx.send(());
            let _ = resume_rx.await;
            Ok(())
        })
        .await
    });
    entered_rx.await?;
    let p = pool.clone();
    let c = calls.clone();
    let second = tokio::spawn(async move {
        acquire_workspace_write_lease_with_cleanup(&p, 817402, "second", 60, async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
    });
    resume_tx.send(()).unwrap();
    assert!(matches!(first.await??, PrincipalLeaseOutcome::Updated(_)));
    assert!(matches!(second.await??, PrincipalLeaseOutcome::Conflict));
    let mut archive = pool.begin().await?;
    sqlx::query(include_str!(
        "../sql/principal_admission/test_lease_archive.sql"
    ))
    .bind(817402_i64)
    .execute(&mut *archive)
    .await?;
    let before = calls.load(Ordering::SeqCst);
    let p = pool.clone();
    let c = calls.clone();
    let mut racing = tokio::spawn(async move {
        release_workspace_write_lease_with_cleanup(&p, 817402, "first", async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut racing)
            .await
            .is_err()
    );
    archive.commit().await?;
    assert!(matches!(racing.await??, PrincipalLeaseOutcome::Conflict));
    assert_eq!(before, calls.load(Ordering::SeqCst));
    pool.close().await;
    Ok(())
}
