use chrono::{DateTime, Utc};
use runner_protocol::RunnerOwner;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::db::types::{ListWorkspaceRunnersParams, Page};

#[derive(Clone, Debug, sqlx::FromRow, ToSchema)]
pub struct WorkspaceRunnerRecord {
    pub runner_id: i64,
    pub runner_manager_id: Option<i64>,
    pub owner_kind: String,
    pub owner_user_id: Option<i64>,
    pub owner_workspace_binding_id: Option<i64>,
    pub owner_application_id: Option<i64>,
    pub owner_principal_digest: Option<String>,
    pub container_name: String,
    pub container_id: Option<String>,
    pub runtime: String,
    pub runtime_class: Option<String>,
    pub image_name: String,
    pub status: String,
    pub network_enabled: bool,
    pub workspace_root: String,
    pub last_active_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SaveWorkspaceRunner<'a> {
    pub runner_manager_id: i64,
    pub owner: &'a RunnerOwner,
    pub container_name: &'a str,
    pub container_id: Option<&'a str>,
    pub runtime: &'a str,
    pub runtime_class: Option<&'a str>,
    pub image_name: &'a str,
    pub status: &'a str,
    pub network_enabled: bool,
    pub workspace_root: &'a str,
    pub last_error: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principal_owner_mapping_validates_identity() {
        let owner = RunnerOwner::ApplicationPrincipal {
            application_id: 3,
            workspace_binding_id: 4,
            principal_digest: "a".repeat(64),
        };
        assert_eq!(
            owner_fields(&owner).unwrap(),
            (
                "application_principal",
                None,
                Some(4),
                Some(3),
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            )
        );
        for owner in [
            RunnerOwner::ApplicationPrincipal {
                application_id: 0,
                workspace_binding_id: 4,
                principal_digest: "a".repeat(64),
            },
            RunnerOwner::ApplicationPrincipal {
                application_id: 3,
                workspace_binding_id: 0,
                principal_digest: "a".repeat(64),
            },
            RunnerOwner::ApplicationPrincipal {
                application_id: 3,
                workspace_binding_id: 4,
                principal_digest: "A".repeat(64),
            },
            RunnerOwner::ExecutionEnvironment {
                environment_id: "e".into(),
                generation: 0,
            },
        ] {
            assert!(owner_fields(&owner).is_err());
        }
    }
}

pub async fn list_workspace_runners(
    pool: &PgPool,
    params: &ListWorkspaceRunnersParams,
) -> anyhow::Result<Page<WorkspaceRunnerRecord>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_workspace_runners.sql"))
        .bind(&params.status)
        .bind(&params.owner_kind)
        .bind(runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, WorkspaceRunnerRecord>(include_str!(
        "../sql/list_workspace_runners.sql"
    ))
    .bind(&params.status)
    .bind(&params.owner_kind)
    .bind(runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn operations_summary(
    pool: &PgPool,
) -> anyhow::Result<(i64, i64, i64, i64, i64, i64, i64)> {
    #[derive(sqlx::FromRow)]
    struct Row {
        active_runners: i64,
        failed_operations: i64,
        archived_workspaces: i64,
        runner_managers_total: i64,
        runner_managers_online: i64,
        runner_managers_offline: i64,
        runner_managers_disabled: i64,
    }
    let row = sqlx::query_as::<_, Row>(include_str!("../sql/operations_summary.sql"))
        .bind(runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
        .fetch_one(pool)
        .await?;
    Ok((
        row.active_runners,
        row.failed_operations,
        row.archived_workspaces,
        row.runner_managers_total,
        row.runner_managers_online,
        row.runner_managers_offline,
        row.runner_managers_disabled,
    ))
}

type OwnerFields<'a> = (
    &'static str,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<&'a str>,
);

fn owner_fields(owner: &RunnerOwner) -> anyhow::Result<OwnerFields<'_>> {
    match owner {
        RunnerOwner::InternalUser { user_id } => Ok(("user", Some(*user_id), None, None, None)),
        RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        } => Ok((
            "workspace_binding",
            None,
            Some(*workspace_binding_id),
            None,
            None,
        )),
        RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } => {
            if *application_id <= 0
                || *workspace_binding_id <= 0
                || principal_digest.len() != 64
                || !principal_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                anyhow::bail!("invalid application principal owner");
            }
            Ok((
                "application_principal",
                None,
                Some(*workspace_binding_id),
                Some(*application_id),
                Some(principal_digest),
            ))
        }
        RunnerOwner::ExecutionEnvironment { .. } => anyhow::bail!(
            "execution environment owners are not persisted through workspace_runners"
        ),
    }
}

pub async fn list_workspace_runner_owners_by_binding(
    pool: &PgPool,
    binding_id: i64,
) -> anyhow::Result<Vec<RunnerOwner>> {
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(include_str!(
        "../sql/list_workspace_runner_owners_by_binding.sql"
    ))
    .bind(binding_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(application_id, workspace_binding_id, principal_digest)| {
            RunnerOwner::ApplicationPrincipal {
                application_id,
                workspace_binding_id,
                principal_digest,
            }
        })
        .collect())
}

pub async fn save_workspace_runner(
    pool: &PgPool,
    input: SaveWorkspaceRunner<'_>,
) -> anyhow::Result<WorkspaceRunnerRecord> {
    let (
        owner_kind,
        owner_user_id,
        owner_workspace_binding_id,
        owner_application_id,
        owner_principal_digest,
    ) = owner_fields(input.owner)?;

    sqlx::query_as::<_, WorkspaceRunnerRecord>(include_str!("../sql/save_workspace_runner.sql"))
        .bind(input.runner_manager_id)
        .bind(owner_kind)
        .bind(owner_user_id)
        .bind(owner_workspace_binding_id)
        .bind(owner_application_id)
        .bind(owner_principal_digest)
        .bind(input.container_name)
        .bind(input.container_id)
        .bind(input.runtime)
        .bind(input.runtime_class)
        .bind(input.image_name)
        .bind(input.status)
        .bind(input.network_enabled)
        .bind(input.workspace_root)
        .bind(input.last_error)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn report_workspace_runner(
    pool: &PgPool,
    runner_manager_id: i64,
    event: &runner_protocol::RunnerLifecycleEvent,
) -> anyhow::Result<()> {
    let (
        owner_kind,
        owner_user_id,
        owner_workspace_binding_id,
        owner_application_id,
        owner_principal_digest,
    ) = owner_fields(&event.owner)?;
    let status = event.status.as_str();
    if status == "running" {
        let input = SaveWorkspaceRunner {
            runner_manager_id,
            owner: &event.owner,
            container_name: &event.container_name,
            container_id: event.container_id.as_deref(),
            runtime: event.runtime.as_deref().unwrap_or("docker"),
            runtime_class: event.runtime_class.as_deref(),
            image_name: event.image_name.as_deref().unwrap_or("unknown"),
            status,
            network_enabled: event.network_enabled.unwrap_or(false),
            workspace_root: event.workspace_root.as_deref().unwrap_or(""),
            last_error: event.last_error.as_deref(),
        };
        save_workspace_runner(pool, input).await?;
        return Ok(());
    }

    sqlx::query(include_str!("../sql/report_workspace_runner.sql"))
        .bind(runner_manager_id)
        .bind(&event.container_name)
        .bind(owner_kind)
        .bind(status)
        .bind(event.container_id.as_deref())
        .bind(event.last_error.as_deref())
        .bind(owner_user_id)
        .bind(owner_workspace_binding_id)
        .bind(owner_application_id)
        .bind(owner_principal_digest)
        .execute(pool)
        .await?;
    Ok(())
}
