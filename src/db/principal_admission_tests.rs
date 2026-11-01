use super::*;
use runner_protocol::workspace_principal::application_principal_owner;

#[test]
fn identity_and_container_validation_fail_closed() {
    let owner = application_principal_owner(1, 2, "u", "w").unwrap();
    assert!(identity(&owner).is_ok());
    assert!(identity(&RunnerOwner::InternalUser { user_id: 1 }).is_err());
    assert!(
        identity(&RunnerOwner::ApplicationPrincipal {
            application_id: 1,
            workspace_binding_id: 2,
            principal_digest: "A".repeat(64)
        })
        .is_err()
    );
    for (manager, id) in [
        (0, "abc".to_string()),
        (1, String::new()),
        (1, "a b".into()),
        (1, "a\nb".into()),
        (1, "x".repeat(257)),
    ] {
        assert!(check_manager_container(manager, &id).is_err());
    }
    assert!(check_manager_container(1, &"a".repeat(256)).is_ok());
}

#[tokio::test]
#[ignore = "requires explicitly task-owned disposable PostgreSQL and migrated schema"]
async fn isolated_storage_races_constraints_and_binding_lock() -> anyhow::Result<()> {
    use std::time::Duration;
    let url = std::env::var("P4J1_TEST_DATABASE_URL")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    sqlx::raw_sql(include_str!("../sql/principal_admission/test_seed.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_constraints.sql"
    ))
    .execute(&pool)
    .await?;
    let a = application_principal_owner(817001, 817002, "a", "w").unwrap();
    let b = application_principal_owner(817001, 817002, "b", "w").unwrap();
    assert!(matches!(
        admit(&pool, 817003, &a, "container-a").await?,
        AdmissionOutcome::Admitted(_)
    ));
    assert!(matches!(
        admit(&pool, 817003, &a, "container-a").await?,
        AdmissionOutcome::Admitted(_)
    ));
    assert!(matches!(
        admit(&pool, 817004, &a, "container-a").await?,
        AdmissionOutcome::Admitted(_)
    ));
    assert!(matches!(
        admit(&pool, 817003, &a, "container-b").await?,
        AdmissionOutcome::Admitted(_)
    ));
    assert_eq!(list_active_by_owner(&pool, &a).await?.len(), 3);
    assert!(!mark_stopped(&pool, 817003, &a, "wrong").await?);
    assert!(!mark_stopped(&pool, 817003, &b, "container-a").await?);
    fence(&pool, &a).await?;
    fence(&pool, &a).await?;
    assert_eq!(
        admit(&pool, 817003, &a, "container-c").await?,
        AdmissionOutcome::Fenced
    );
    assert!(mark_stopped(&pool, 817003, &a, "container-a").await?);
    assert_eq!(list_active_by_owner(&pool, &a).await?.len(), 2);
    assert!(
        list_fences_by_binding(&pool, 817002)
            .await?
            .iter()
            .any(|f| f.owner_key == a.stable_key())
    );
    assert_eq!(
        admit(&pool, 817003, &a, "container-a").await?,
        AdmissionOutcome::Fenced
    );
    fence(&pool, &b).await?;
    assert_eq!(
        admit(&pool, 817003, &b, "container-b").await?,
        AdmissionOutcome::Fenced
    );
    for i in 0..40 {
        let owner = application_principal_owner(817001, 817002, &format!("race-{i}"), "w").unwrap();
        let (admission, stop) = tokio::join!(
            admit(&pool, 817003, &owner, "race-container"),
            fence(&pool, &owner)
        );
        let admission = admission?;
        stop?;
        let visible = list_active_by_owner(&pool, &owner).await?;
        match admission {
            AdmissionOutcome::Admitted(_) => assert_eq!(visible.len(), 1),
            AdmissionOutcome::Fenced => assert!(visible.is_empty()),
        }
        assert_eq!(
            admit(&pool, 817003, &owner, "new").await?,
            AdmissionOutcome::Fenced
        );
    }
    let mut locked = pool.begin().await?;
    sqlx::query(include_str!(
        "../sql/principal_admission/test_lock_binding.sql"
    ))
    .execute(&mut *locked)
    .await?;
    let owner = application_principal_owner(817001, 817002, "lock-proof", "w").unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        admit(&pool, 817003, &owner, "lock-container").await?;
        fence(&pool, &owner).await
    })
    .await??;
    let mut probe = pool.begin().await?;
    sqlx::query(include_str!(
        "../sql/principal_admission/test_lock_timeout.sql"
    ))
    .execute(&mut *probe)
    .await?;
    let error = sqlx::query(include_str!(
        "../sql/principal_admission/test_registry_fk.sql"
    ))
    .execute(&mut *probe)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("55P03")
    );
    probe.rollback().await?;
    locked.rollback().await?;
    assert!(
        list_active_by_binding(&pool, 817002)
            .await?
            .iter()
            .any(|r| r.owner_key == owner.stable_key())
    );
    pool.close().await;
    Ok(())
}
