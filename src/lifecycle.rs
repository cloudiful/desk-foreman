use std::{fs, time::Duration};

mod guard;

#[cfg(test)]
mod retention_tests;

use crate::{
    AppState,
    db::{self, audit::AuditLogEntry},
};
use chrono::{Duration as ChronoDuration, Utc};

pub const GIT_JOB_EXPIRY_INTERVAL: Duration = Duration::from_secs(60);

/// Counts from one bounded Git-expiry tick. The production loop logs these
/// fixed values and never exposes a job id, token, credential, or row body.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GitExpiryReport {
    pub expired_jobs: usize,
    pub pruned_credentials: usize,
}

pub fn spawn_janitor(state: AppState) {
    let workspace_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(error) = cleanup_archived_workspaces(&workspace_state).await {
                tracing::warn!(%error, "workspace janitor failed");
            }
        }
    });
    tokio::spawn(async move {
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let _ = run_git_job_expiry(state, shutdown_rx, GIT_JOB_EXPIRY_INTERVAL).await;
    });
}

/// The actual production Git-expiry loop, shared by `spawn_janitor` and tests.
///
/// The first tick is immediate and later ticks use `interval`. Tests pass an
/// explicit shutdown receiver and short interval so the loop is bounded and
/// joined instead of running detached over the issue database.
#[doc(hidden)]
pub async fn run_git_job_expiry(
    state: AppState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    interval: Duration,
) -> GitExpiryReport {
    let mut ticker = tokio::time::interval(interval);
    let mut report = GitExpiryReport::default();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let tick = expire_git_jobs_once(&state).await;
                report.expired_jobs += tick.expired_jobs;
                report.pruned_credentials += tick.pruned_credentials;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    report
}

async fn expire_git_jobs_once(state: &AppState) -> GitExpiryReport {
    let mut report = GitExpiryReport::default();
    match db::workspace_git_jobs::lifecycle::expire_git_jobs(&state.db).await {
        Ok(expired) => {
            report.expired_jobs = expired.len();
            if !expired.is_empty() {
                tracing::info!(expired = expired.len(), "expired durable git jobs");
            }
        }
        Err(error) => tracing::warn!(%error, "git job expiry failed"),
    }
    report.pruned_credentials = state.git_job_credentials.prune_expired();
    report
}

/// One archived-workspace sweep.
///
/// Exposed so a test can drive the real janitor path — lock, guard, filesystem
/// cleanup, delete — instead of asserting on a re-implementation of it.
pub async fn cleanup_archived_workspaces(state: &AppState) -> anyhow::Result<()> {
    let before = Utc::now()
        - ChronoDuration::from_std(state.config.workspace_retention)
            .map_err(|_| anyhow::anyhow!("invalid workspace retention"))?;
    let archived = db::queries::list_archived_workspace_bindings(&state.db, before).await?;
    for binding in archived {
        if let Err(error) = cleanup_archived_binding(state, before, &binding).await {
            tracing::warn!(
                workspace_binding_id = binding.workspace_binding_id,
                %error,
                "failed to clean up archived workspace"
            );
        }
    }
    Ok(())
}

/// Clean one archived binding while holding its row lock through the check,
/// filesystem effect and delete. Retained references skip the binding and keep
/// its directory. The filesystem removal happens before the guarded row delete,
/// so an error between those steps is reported to the sweep caller rather than
/// treated as an atomic directory-and-row update.
async fn cleanup_archived_binding(
    state: &AppState,
    before: chrono::DateTime<Utc>,
    binding: &crate::db::types::WorkspaceBindingResponse,
) -> anyhow::Result<()> {
    // Take the binding row lock, then re-read the native-task, Git-job and
    // retained-reference guards, and hold it through the filesystem cleanup and
    // the delete. An admitted task or durable evidence owns this workspace, so
    // an archived binding that still has one is skipped rather than deleted
    // out from under it.
    let mut cleanup =
        guard::lock_binding_for_cleanup(&state.db, binding.workspace_binding_id).await?;
    if cleanup.should_skip() {
        tracing::info!(
            workspace_binding_id = binding.workspace_binding_id,
            "janitor skipped an archived workspace a task or retained evidence still owns"
        );
        cleanup.release().await?;
        return Ok(());
    }
    let Some(current) =
        db::queries::find_workspace_binding_by_id(&state.db, binding.workspace_binding_id).await?
    else {
        cleanup.release().await?;
        return Ok(());
    };
    if current.is_active
        || current.lifecycle_state != "archived"
        || current
            .archived_at
            .is_none_or(|archived| archived >= before)
    {
        cleanup.release().await?;
        return Ok(());
    }
    crate::runner::principal_stop::cleanup_binding_admitted_work(
        &state.db,
        state.runner.as_ref(),
        binding.workspace_binding_id,
    )
    .await?;
    let owner = runner_protocol::RunnerOwner::WorkspaceBinding {
        workspace_binding_id: binding.workspace_binding_id,
    };
    state.runner.cleanup_runner_owner(owner.clone()).await?;
    if let Err(error) = fs::remove_dir_all(&current.workspace_root)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.into());
    }
    // The delete runs on the guarded transaction, so the row cannot be removed
    // outside the window this lock closes; the lock is held through the
    // filesystem cleanup and the delete, then released by commit.
    let deleted = crate::db::workspace_bindings::native_task_guard::delete_binding(
        cleanup.transaction_mut(),
        binding.workspace_binding_id,
    )
    .await?;
    cleanup.release().await?;
    if deleted.is_some() {
        db::queries::record_audit(
            &state.db,
            AuditLogEntry {
                actor_user_id: None,
                actor_application_id: None,
                actor_type: "system",
                action: "workspace.delete",
                target_type: "workspace_binding",
                target_id: &binding.workspace_binding_id.to_string(),
                // The binding row no longer exists, so the audit row cannot
                // carry a foreign key to it; `target_id` keeps the identifier
                // for the audit trail.
                workspace_binding_id: None,
                external_user_id: Some(&binding.external_user_id),
                payload: serde_json::json!({}),
                request_id: None,
                session_id: None,
                duration_ms: None,
                status: Some("success"),
            },
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::GIT_JOB_EXPIRY_INTERVAL;

    #[test]
    fn git_job_expiry_interval_is_sixty_seconds() {
        assert_eq!(GIT_JOB_EXPIRY_INTERVAL, std::time::Duration::from_secs(60));
    }
}
