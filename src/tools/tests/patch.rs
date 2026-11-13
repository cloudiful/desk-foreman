use std::{
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tempfile::tempdir;

use crate::{
    actor::ActorContext,
    db,
    runner::{RunnerBroker, RunnerService},
    tools::{
        ToolError,
        common::{
            PatchEffectOutput, PatchOperationOutput, PatchResultOutput, PatchStatusOutput,
            PatchWritesOutput,
        },
        params::ApplyPatchParams,
        shared,
    },
};

use super::{FakeRunnerService, app_state_with_runner, resource_workspace_actor};

const APP_ID: i64 = 991001;
const BINDING_ID: i64 = 991002;
const PRINCIPAL: &str = "patch-fence-test";
const LEASE_OWNER: &str = "patch-fence-test";

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database_pool() -> anyhow::Result<PgPool> {
    let url = std::env::var("DF_PATCH_TEST_DATABASE_URL")?;
    let options = PgConnectOptions::from_str(&url)?;
    let host = options.get_host();
    let database = options.get_database().unwrap_or_default();
    anyhow::ensure!(
        host == "127.0.0.1" || host == "localhost",
        "DF_PATCH_TEST_DATABASE_URL must point at loopback"
    );
    anyhow::ensure!(
        database == "df_patch_test" || database.starts_with("df_patch_test_"),
        "DF_PATCH_TEST_DATABASE_URL database must be df_patch_test or an owned df_patch_test_* suffix"
    );
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await?;
    db::migrate(&pool).await?;
    Ok(pool)
}

async fn cleanup(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(include_str!("../../sql/patch/test_patch_cleanup.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(include_str!("../../sql/patch/test_fence_binding.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn audit_count(pool: &PgPool) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>(include_str!("../../sql/patch/test_patch_audit_count.sql"))
            .fetch_one(pool)
            .await?,
    )
}

async fn wait_for_audit(pool: &PgPool, expected: i64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = audit_count(pool).await.expect("audit count");
        if count >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "detached patch audit did not complete before the deadline"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn state_with_pool(root: &Path, pool: PgPool) -> crate::AppState {
    let runner = FakeRunnerService::new() as Arc<dyn RunnerService>;
    let mut state = app_state_with_runner(root.to_path_buf(), runner);
    state.db = pool.clone();
    state.task_runtime =
        std::sync::Arc::new(crate::db::task_runtime::TaskRuntime::new(pool.clone()));
    state.runner_broker = RunnerBroker::new(pool);
    state
}

fn patch_actor(root: &Path) -> ActorContext {
    let mut actor = resource_workspace_actor(root, Some(LEASE_OWNER));
    actor.principal_id = PRINCIPAL.to_string();
    actor.workspace_binding_id = BINDING_ID;
    if let Some(binding) = actor.workspace_binding.as_mut() {
        binding.workspace_binding_id = BINDING_ID;
        binding.application_id = APP_ID;
        binding.workspace_root = root.to_string_lossy().to_string();
        binding.lifecycle_state = "active".to_string();
        binding.is_active = true;
        binding.write_lease_owner = Some(LEASE_OWNER.to_string());
        binding.write_lease_acquired_at = Some(Utc::now());
        binding.write_lease_expires_at = Some(Utc::now() + ChronoDuration::minutes(10));
    }
    actor
}

fn params(patch: &str) -> ApplyPatchParams {
    ApplyPatchParams {
        patch_text: patch.to_string(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires owned DF_PATCH_TEST_DATABASE_URL"]
async fn post_write_commit_failure_reports_all_effects_and_no_audit() -> anyhow::Result<()> {
    let _guard = DB_LOCK.lock().await;
    let pool = database_pool().await?;
    cleanup(&pool).await?;
    seed(&pool).await?;
    let temp = tempdir()?;
    let state = state_with_pool(temp.path(), pool.clone());
    let actor = patch_actor(temp.path());

    let output = shared::apply_patch_with_commit_failure(
        &state,
        &actor,
        &params("*** Begin Patch\n*** Add File: fenced.txt\n+committed content\n*** End Patch"),
    )
    .await
    .expect("patch domain result");

    let PatchResultOutput::Incomplete(incomplete) = &output else {
        panic!("fence failure after writes must be incomplete");
    };
    assert_eq!(
        incomplete.writes,
        crate::tools::common::PatchWritesOutput::All
    );
    assert_eq!(incomplete.changes.len(), 1);
    assert_eq!(incomplete.changes[0].operation, PatchOperationOutput::Add);
    assert_eq!(incomplete.changes[0].status, PatchStatusOutput::Applied);
    assert_eq!(incomplete.changes[0].effect, PatchEffectOutput::Changed);
    assert!(incomplete.changes[0].error.is_none());
    assert_eq!(
        incomplete.diagnostic.code,
        crate::tools::common::PatchCodeOutput::PatchFenceFailed
    );
    assert_eq!(
        incomplete.diagnostic.stage,
        crate::tools::common::PatchStageOutput::Fence
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("fenced.txt"))?,
        "committed content\n"
    );
    assert_eq!(
        audit_count(&pool).await?,
        0,
        "a failed fence commit must not spawn an audit claim"
    );
    cleanup(&pool).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires owned DF_PATCH_TEST_DATABASE_URL"]
async fn noop_fence_failure_reports_no_writes_and_no_audit() -> anyhow::Result<()> {
    let _guard = DB_LOCK.lock().await;
    let pool = database_pool().await?;
    cleanup(&pool).await?;
    seed(&pool).await?;
    let temp = tempdir()?;
    std::fs::write(temp.path().join("noop.txt"), "same\n")?;
    let state = state_with_pool(temp.path(), pool.clone());
    let actor = patch_actor(temp.path());

    let output = shared::apply_patch_with_commit_failure(
        &state,
        &actor,
        &params("*** Begin Patch\n*** Update File: noop.txt\n@@\n-same\n+same\n*** End Patch"),
    )
    .await
    .expect("patch domain result");

    let PatchResultOutput::Incomplete(incomplete) = &output else {
        panic!("fence failure after a no-op update must be incomplete");
    };
    assert_eq!(incomplete.writes, PatchWritesOutput::None);
    assert_eq!(incomplete.changes.len(), 1);
    assert_eq!(
        incomplete.changes[0].operation,
        PatchOperationOutput::Update
    );
    assert_eq!(incomplete.changes[0].status, PatchStatusOutput::Applied);
    assert_eq!(incomplete.changes[0].effect, PatchEffectOutput::Unchanged);
    assert!(incomplete.changes[0].error.is_none());
    assert_eq!(
        incomplete.diagnostic.code,
        crate::tools::common::PatchCodeOutput::PatchFenceFailed
    );
    assert_eq!(
        incomplete.diagnostic.stage,
        crate::tools::common::PatchStageOutput::Fence
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("noop.txt"))?,
        "same\n"
    );
    assert_eq!(
        audit_count(&pool).await?,
        0,
        "a failed fence commit must not spawn an audit claim"
    );
    cleanup(&pool).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires owned DF_PATCH_TEST_DATABASE_URL"]
async fn real_commit_returns_applied_and_waits_for_scoped_audit() -> anyhow::Result<()> {
    let _guard = DB_LOCK.lock().await;
    let pool = database_pool().await?;
    cleanup(&pool).await?;
    seed(&pool).await?;
    let temp = tempdir()?;
    let state = state_with_pool(temp.path(), pool.clone());
    let actor = patch_actor(temp.path());

    let output = shared::apply_patch(
        &state,
        &actor,
        &params("*** Begin Patch\n*** Add File: control.txt\n+control content\n*** End Patch"),
    )
    .await
    .expect("patch domain result");
    let PatchResultOutput::Applied(applied) = &output else {
        panic!("real commit must be applied");
    };
    assert_eq!(applied.changes.len(), 1);
    assert_eq!(applied.changes[0].status, PatchStatusOutput::Applied);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("control.txt"))?,
        "control content\n"
    );

    wait_for_audit(&pool, 1).await;
    assert_eq!(
        audit_count(&pool).await?,
        1,
        "exactly one scoped audit row is expected"
    );
    cleanup(&pool).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires owned DF_PATCH_TEST_DATABASE_URL"]
async fn missing_binding_denies_before_writes_and_audits_nothing() -> anyhow::Result<()> {
    let _guard = DB_LOCK.lock().await;
    let pool = database_pool().await?;
    cleanup(&pool).await?;
    let temp = tempdir()?;
    let state = state_with_pool(temp.path(), pool.clone());
    let actor = patch_actor(temp.path());

    let error = shared::apply_patch(
        &state,
        &actor,
        &params("*** Begin Patch\n*** Add File: denied.txt\n+denied\n*** End Patch"),
    )
    .await
    .expect_err("a missing binding must deny before writes");
    assert!(matches!(error, ToolError::Forbidden(_)));
    assert!(!temp.path().join("denied.txt").exists());
    assert_eq!(audit_count(&pool).await?, 0);
    Ok(())
}
