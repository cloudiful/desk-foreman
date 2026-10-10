use std::{fs, time::Duration};

mod guard;

use crate::{
    AppState,
    db::{self, audit::AuditLogEntry},
};
use chrono::{Duration as ChronoDuration, Utc};

pub fn spawn_janitor(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(error) = cleanup_archived_workspaces(&state).await {
                tracing::warn!(%error, "workspace janitor failed");
            }
        }
    });
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
        // Take the binding row lock, then re-read the native-task guard, and
        // hold it through the filesystem cleanup and the delete. An admitted
        // task owns this workspace, so an archived binding that still has one
        // is skipped rather than deleted out from under it.
        let mut cleanup =
            guard::lock_binding_for_cleanup(&state.db, binding.workspace_binding_id).await?;
        if cleanup.should_skip() {
            tracing::info!(
                workspace_binding_id = binding.workspace_binding_id,
                "janitor skipped an archived workspace a native task still owns"
            );
            cleanup.release().await?;
            continue;
        }
        let owner = runner_protocol::RunnerOwner::WorkspaceBinding {
            workspace_binding_id: binding.workspace_binding_id,
        };
        if let Err(error) = state.runner.cleanup_runner_owner(owner.clone()).await {
            tracing::warn!(
                workspace_binding_id = binding.workspace_binding_id,
                %error,
                "failed to clean up archived workspace runner"
            );
            continue;
        }
        if let Err(error) = fs::remove_dir_all(&binding.workspace_root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                workspace_binding_id = binding.workspace_binding_id,
                %error,
                "failed to remove archived workspace directory"
            );
            continue;
        }
        // The delete runs on the guarded transaction, so the row cannot be
        // removed outside the window this lock closes; the lock is held through
        // the filesystem cleanup and the delete, then released by commit.
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
                    // carry a foreign key to it; `target_id` keeps the
                    // identifier for the audit trail.
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
    }
    Ok(())
}
