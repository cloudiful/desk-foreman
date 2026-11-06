use super::*;
use chrono::Utc;

use crate::lease_test_support as support;

#[test]
fn request_rejects_injection_and_invalid_containers() {
    let owner = application_principal_owner(1, 2, "user", "workspace").unwrap();
    assert!(
        serde_json::from_value::<PrincipalAdmissionRequest>(
            serde_json::json!({"owner": owner, "container_id":"id", "runner_manager_id":1})
        )
        .is_err()
    );
    let mut injected = serde_json::to_value(&owner).unwrap();
    injected["ApplicationPrincipal"]["runner_manager_id"] = serde_json::json!(42);
    assert!(
        serde_json::from_value::<PrincipalAdmissionRequest>(
            serde_json::json!({"owner": injected, "container_id":"id"})
        )
        .is_err()
    );
    for container_id in ["".to_string(), "bad id".into(), "x".repeat(257)] {
        assert!(
            !PrincipalAdmissionRequest {
                owner: owner.clone(),
                container_id
            }
            .is_valid()
        );
    }
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4j2 database"]
async fn endpoint_auth_persistence_and_binding_lock() -> anyhow::Result<()> {
    use axum::http::header::AUTHORIZATION;
    let url = std::env::var("P4J2_TEST_DATABASE_URL")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_endpoint_seed.sql"
    ))
    .execute(&pool)
    .await?;
    for (id, token, enabled) in [(817103, "p4j2-ok", true), (817104, "p4j2-disabled", false)] {
        sqlx::query(include_str!(
            "../../sql/principal_admission/test_endpoint_manager.sql"
        ))
        .bind(id)
        .bind(token)
        .bind(crate::auth::hash_bearer_token(token))
        .bind(enabled)
        .execute(&pool)
        .await?;
    }
    let mut state = support::fake_state(support::FakeRunnerService::new());
    state.db = pool.clone();
    let mut headers = HeaderMap::new();
    let owner = application_principal_owner(817101, 817102, "principal", "workspace").unwrap();
    let request = PrincipalAdmissionRequest {
        owner: owner.clone(),
        container_id: "container-immutable".into(),
    };
    assert!(matches!(
        principal_admission(
            State(state.clone()),
            headers.clone(),
            Ok(Json(request.clone()))
        )
        .await,
        Err(AppError::Unauthorized(_))
    ));
    headers.insert(AUTHORIZATION, "Bearer p4j2-disabled".parse()?);
    assert!(matches!(
        principal_admission(
            State(state.clone()),
            headers.clone(),
            Ok(Json(request.clone()))
        )
        .await,
        Err(AppError::Forbidden(_))
    ));
    headers.insert(AUTHORIZATION, "Bearer invalid".parse()?);
    assert!(matches!(
        principal_admission(
            State(state.clone()),
            headers.clone(),
            Ok(Json(request.clone()))
        )
        .await,
        Err(AppError::NotFound(_))
    ));
    headers.insert(AUTHORIZATION, "Bearer p4j2-ok".parse()?);
    let mut invalid = request.clone();
    invalid.owner = RunnerOwner::InternalUser { user_id: 1 };
    assert!(matches!(
        principal_admission(State(state.clone()), headers.clone(), Ok(Json(invalid))).await,
        Err(AppError::BadRequest(_))
    ));
    let mut missing = request.clone();
    missing.owner = application_principal_owner(817101, 817999, "principal", "workspace").unwrap();
    assert!(matches!(
        principal_admission(State(state.clone()), headers.clone(), Ok(Json(missing))).await,
        Err(AppError::NotFound(_))
    ));
    for other in [
        application_principal_owner(817999, 817102, "principal", "workspace").unwrap(),
        application_principal_owner(817101, 817102, "other", "workspace").unwrap(),
    ] {
        let mut mismatch = request.clone();
        mismatch.owner = other;
        assert!(matches!(
            principal_admission(State(state.clone()), headers.clone(), Ok(Json(mismatch))).await,
            Err(AppError::Conflict(_))
        ));
    }
    let mut locked = pool.begin().await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_endpoint_lock.sql"
    ))
    .execute(&mut *locked)
    .await?;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        principal_admission(
            State(state.clone()),
            headers.clone(),
            Ok(Json(request.clone())),
        ),
    )
    .await?
    .expect("admission succeeds");
    assert_eq!(result, StatusCode::NO_CONTENT);
    let rows = principal_admission::list_active_by_owner(&pool, &owner).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].runner_manager_id, 817103);
    assert_eq!(
        principal_admission(
            State(state.clone()),
            headers.clone(),
            Ok(Json(request.clone()))
        )
        .await
        .expect("duplicate succeeds"),
        StatusCode::NO_CONTENT
    );
    principal_admission::fence(&pool, &owner).await?;
    assert!(matches!(
        principal_admission(State(state), headers, Ok(Json(request))).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &owner)
            .await?
            .len(),
        1
    );
    locked.rollback().await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4k4 PostgreSQL"]
async fn real_resource_marker_admission_and_no_evidence_fence() -> anyhow::Result<()> {
    use crate::runner::principal_stop::{AdmittedWorkCleanup, cleanup_binding_admitted_work};
    use axum::http::header::AUTHORIZATION;
    use std::sync::Arc;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&std::env::var("P4K4_TEST_DATABASE_URL")?)
        .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_real_resource_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_endpoint_manager.sql"
    ))
    .bind(817503_i64)
    .bind("p4k4-manager")
    .bind(crate::auth::hash_bearer_token("p4k4-token"))
    .bind(true)
    .execute(&pool)
    .await?;
    let temp = tempfile::tempdir()?;
    let runner = support::FakeRunnerService::new();
    let mut state = support::fake_state(runner.clone());
    state.db = pool.clone();
    let mut config = (*state.config).clone();
    config.workspace_root = temp.path().into();
    state.config = Arc::new(config);
    let app = crate::db::queries::find_application_by_id(&pool, 817501)
        .await?
        .unwrap();
    let binding = crate::auth::identity::resolve_or_create_resource_binding(
        &state,
        &app,
        "artifact",
        "one",
        "artifact:one",
    )
    .await
    .expect("resource binding");
    assert_eq!(binding.external_user_id, "__resource__");
    crate::db::queries::acquire_workspace_write_lease(
        &pool,
        binding.workspace_binding_id,
        "principal-a",
        600,
    )
    .await?
    .expect("lease acquired");
    let owner = application_principal_owner(
        817501,
        binding.workspace_binding_id,
        "principal-a",
        "artifact:one",
    )
    .unwrap();
    let other = application_principal_owner(
        817501,
        binding.workspace_binding_id,
        "principal-b",
        "artifact:one",
    )
    .unwrap();
    assert_ne!(owner, other);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, "Bearer p4k4-token".parse()?);
    let request = PrincipalAdmissionRequest {
        owner: owner.clone(),
        container_id: "real-id".into(),
    };
    let mut wrong = request.clone();
    wrong.owner = other;
    assert!(matches!(
        principal_admission(State(state.clone()), headers.clone(), Ok(Json(wrong))).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(
        principal_admission(State(state.clone()), headers, Ok(Json(request)))
            .await
            .expect("admitted"),
        StatusCode::NO_CONTENT
    );
    let empty = crate::auth::identity::resolve_or_create_resource_binding(
        &state,
        &app,
        "artifact",
        "two",
        "artifact:two",
    )
    .await
    .expect("second resource binding");
    assert_eq!(empty.external_user_id, "__resource__");
    crate::db::queries::acquire_workspace_write_lease(
        &pool,
        empty.workspace_binding_id,
        "principal-b",
        600,
    )
    .await?
    .expect("lease acquired");
    let empty_owner = application_principal_owner(
        817501,
        empty.workspace_binding_id,
        "principal-b",
        "artifact:two",
    )
    .unwrap();
    assert_eq!(
        cleanup_binding_admitted_work(&pool, runner.as_ref(), empty.workspace_binding_id).await?,
        AdmittedWorkCleanup::NoActiveAdmissions
    );
    assert_eq!(
        principal_admission::admit(&pool, 817503, &empty_owner, "late-id").await?,
        principal_admission::AdmissionOutcome::Fenced
    );
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &owner)
            .await?
            .len(),
        1
    );
    let race = crate::auth::identity::resolve_or_create_resource_binding(
        &state,
        &app,
        "artifact",
        "race",
        "artifact:race",
    )
    .await
    .expect("race resource binding");
    crate::db::queries::acquire_workspace_write_lease(
        &pool,
        race.workspace_binding_id,
        "principal-c",
        600,
    )
    .await?
    .expect("lease acquired");
    let racing_owner = application_principal_owner(
        817501,
        race.workspace_binding_id,
        "principal-c",
        "artifact:race",
    )
    .unwrap();
    let (admission, fence) = tokio::join!(
        principal_admission::admit(&pool, 817503, &racing_owner, "race-id"),
        principal_admission::fence(&pool, &racing_owner)
    );
    let admission = admission?;
    fence?;
    let visible = principal_admission::list_active_by_owner(&pool, &racing_owner).await?;
    match admission {
        principal_admission::AdmissionOutcome::Admitted(_) => assert_eq!(visible.len(), 1),
        principal_admission::AdmissionOutcome::Fenced => assert!(visible.is_empty()),
    }
    assert_eq!(
        principal_admission::admit(&pool, 817503, &racing_owner, "later").await?,
        principal_admission::AdmissionOutcome::Fenced
    );
    pool.close().await;
    Ok(())
}

#[test]
fn lease_identity_must_match_current_resource_binding() {
    let owner = application_principal_owner(1, 2, "user", "workspace").unwrap();
    let mut binding = AdmissionBinding {
        workspace_binding_id: 2,
        application_id: 1,
        external_user_id: "__resource__".into(),
        workspace_key: "workspace".into(),
        is_active: true,
        lifecycle_state: "active".into(),
        archived_at: None,
        resource_kind: Some("project".into()),
        resource_id: Some("1".into()),
        write_lease_owner: Some("user".into()),
        lease_unexpired: Some(true),
    };
    assert!(matches_binding(&owner, &binding));
    binding.write_lease_owner = None;
    assert!(!matches_binding(&owner, &binding));
    binding.write_lease_owner = Some("user".into());
    binding.is_active = false;
    assert!(!matches_binding(&owner, &binding));
    binding.is_active = true;
    binding.workspace_binding_id = 3;
    assert!(!matches_binding(&owner, &binding));
    binding.workspace_binding_id = 2;
    binding.lease_unexpired = Some(false);
    assert!(!matches_binding(&owner, &binding));
    binding.lease_unexpired = Some(true);
    binding.write_lease_owner = Some("other".into());
    assert!(!matches_binding(&owner, &binding));
    binding.write_lease_owner = Some("user".into());
    binding.resource_id = None;
    assert!(!matches_binding(&owner, &binding));
    binding.resource_id = Some("1".into());
    binding.archived_at = Some(Utc::now());
    assert!(!matches_binding(&owner, &binding));
    binding.archived_at = None;
    binding.application_id = 3;
    assert!(!matches_binding(&owner, &binding));
}
