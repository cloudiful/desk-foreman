use std::{fs, time::Duration};

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

async fn cleanup_archived_workspaces(state: &AppState) -> anyhow::Result<()> {
    let before = Utc::now()
        - ChronoDuration::from_std(state.config.workspace_retention)
            .map_err(|_| anyhow::anyhow!("invalid workspace retention"))?;
    let archived = db::queries::list_archived_workspace_bindings(&state.db, before).await?;
    for binding in archived {
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
        if db::queries::delete_workspace_binding(&state.db, binding.workspace_binding_id)
            .await?
            .is_some()
        {
            db::queries::record_audit(
                &state.db,
                AuditLogEntry {
                    actor_user_id: None,
                    actor_application_id: None,
                    actor_type: "system",
                    action: "workspace.delete",
                    target_type: "workspace_binding",
                    target_id: &binding.workspace_binding_id.to_string(),
                    workspace_binding_id: Some(binding.workspace_binding_id),
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
