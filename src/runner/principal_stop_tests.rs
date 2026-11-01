use super::*;
use crate::runner::RunnerFuture;
use runner_protocol::{
    CancelSessionRequest, CleanupWorkspaceBindingPrincipalsAck, CommandOutput, ExecRequest,
    InputRequest, RunnerCommandRequest, RunnerSessionStatus, ShellToolOutput,
    WorkspacePrincipalCleanupResult,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct FakeRunner {
    pool: PgPool,
    owner_calls: AtomicUsize,
    binding_calls: AtomicUsize,
    fail: AtomicBool,
}
impl RunnerService for FakeRunner {
    fn exec_shell<'a>(
        &'a self,
        _: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn write_stdin<'a>(
        &'a self,
        _: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn cancel_session<'a>(
        &'a self,
        _: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn run_command<'a>(
        &'a self,
        _: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn cleanup_runner_owner<'a>(&'a self, _: RunnerOwner) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn stop_workspace_principal<'a>(
        &'a self,
        request: StopWorkspacePrincipalRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopWorkspacePrincipalAck>> {
        Box::pin(async move {
            self.owner_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                principal_admission::admit(&self.pool, 7, &request.owner, "late").await?,
                principal_admission::AdmissionOutcome::Fenced
            );
            Ok(StopWorkspacePrincipalAck {
                state: if self.fail.load(Ordering::SeqCst) {
                    StopWorkspacePrincipalState::Failed
                } else {
                    StopWorkspacePrincipalState::Stopped
                },
                detail: None,
            })
        })
    }
    fn cleanup_workspace_binding_principals<'a>(
        &'a self,
        request: CleanupWorkspaceBindingPrincipalsRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CleanupWorkspaceBindingPrincipalsAck>> {
        Box::pin(async move {
            self.binding_calls.fetch_add(1, Ordering::SeqCst);
            for row in principal_admission::list_active_by_binding(
                &self.pool,
                request.workspace_binding_id,
            )
            .await?
            {
                assert_eq!(
                    principal_admission::admit(&self.pool, 7, &admission_owner(&row)?, "late")
                        .await?,
                    principal_admission::AdmissionOutcome::Fenced
                );
            }
            Ok(CleanupWorkspaceBindingPrincipalsAck {
                state: if self.fail.load(Ordering::SeqCst) {
                    StopWorkspacePrincipalState::Failed
                } else {
                    StopWorkspacePrincipalState::Stopped
                },
                owners: Vec::<WorkspacePrincipalCleanupResult>::new(),
                detail: None,
            })
        })
    }
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4k1 PostgreSQL"]
async fn durable_fence_orchestration_and_binding_lock() -> anyhow::Result<()> {
    use std::time::Duration;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("P4K1_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_stop_seed.sql"
    ))
    .execute(&pool)
    .await?;
    let runner = FakeRunner {
        pool: pool.clone(),
        owner_calls: AtomicUsize::new(0),
        binding_calls: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    };
    let owner = application_principal_owner(817201, 817202, "current", "project:one").unwrap();
    let old = application_principal_owner(817201, 817202, "old", "project:one").unwrap();
    let other = application_principal_owner(817201, 817203, "other", "project:two").unwrap();
    let empty = application_principal_owner(817201, 817204, "empty", "project:three").unwrap();
    assert_eq!(
        cleanup_binding_admitted_work(&pool, &runner, 817204).await?,
        AdmittedWorkCleanup::NoActiveAdmissions
    );
    assert_eq!(runner.binding_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        principal_admission::admit(&pool, 1, &empty, "late").await?,
        principal_admission::AdmissionOutcome::Fenced
    );
    principal_admission::admit(&pool, 10, &owner, "owner-a").await?;
    principal_admission::admit(&pool, 11, &owner, "owner-b").await?;
    principal_admission::admit(&pool, 10, &old, "old-a").await?;
    principal_admission::admit(&pool, 10, &other, "other-a").await?;
    let request = StopWorkspacePrincipalRequest {
        owner: owner.clone(),
        principal_id: "current".into(),
    };
    runner.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        stop_owner(&pool, &runner, request.clone()).await?.state,
        StopWorkspacePrincipalState::Failed
    );
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &owner)
            .await?
            .len(),
        2
    );
    runner.fail.store(false, Ordering::SeqCst);
    let rows = principal_admission::list_active_by_owner(&pool, &owner).await?;
    assert_eq!(
        rows.iter()
            .map(|r| r.container_id.as_str())
            .collect::<Vec<_>>(),
        ["owner-a", "owner-b"]
    );
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_stop_fail_trigger.sql"
    ))
    .execute(&pool)
    .await?;
    let error = stop_owner(&pool, &runner, request.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected second mark failure"));
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &owner)
            .await?
            .len(),
        2
    );
    assert_eq!(
        principal_admission::admit(&pool, 1, &owner, "after-failed-mark").await?,
        principal_admission::AdmissionOutcome::Fenced
    );
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_stop_drop_trigger.sql"
    ))
    .execute(&pool)
    .await?;
    assert_eq!(
        stop_owner(&pool, &runner, request.clone()).await?.state,
        StopWorkspacePrincipalState::Stopped
    );
    assert!(
        principal_admission::list_active_by_owner(&pool, &owner)
            .await?
            .is_empty()
    );
    principal_admission::mark_stopped_batch(&pool, &rows).await?;
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &other)
            .await?
            .len(),
        1
    );
    assert_eq!(
        stop_owner(&pool, &runner, request).await?.state,
        StopWorkspacePrincipalState::Stopped
    );
    assert_eq!(runner.owner_calls.load(Ordering::SeqCst), 4);
    let mut locked = pool.begin().await?;
    sqlx::query(include_str!(
        "../sql/principal_admission/test_stop_lock.sql"
    ))
    .execute(&mut *locked)
    .await?;
    runner.fail.store(true, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            cleanup_binding_admitted_work(&pool, &runner, 817202)
        )
        .await?
        .is_err()
    );
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &old)
            .await?
            .len(),
        1
    );
    assert_eq!(
        principal_admission::admit(&pool, 1, &old, "late").await?,
        principal_admission::AdmissionOutcome::Fenced
    );
    runner.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(2),
            cleanup_binding_admitted_work(&pool, &runner, 817202)
        )
        .await??,
        AdmittedWorkCleanup::ConfirmedStopped
    );
    locked.rollback().await?;
    assert!(
        principal_admission::list_active_by_owner(&pool, &old)
            .await?
            .is_empty()
    );
    assert_eq!(
        principal_admission::list_active_by_owner(&pool, &other)
            .await?
            .len(),
        1
    );
    pool.close().await;
    Ok(())
}
