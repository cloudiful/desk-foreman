use chrono::{DateTime, Utc};
use runner_protocol::RunnerOwner;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, ExprTrait},
};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::db::native::{runner_manager, workspace_runner};
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
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

fn native(pool: &PgPool) -> DatabaseConnection {
    DatabaseConnection::from(pool.clone())
}

fn status_expr() -> Expr {
    let ttl = runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64;
    Expr::case(
        Condition::all()
            .add(workspace_runner::Column::Status.eq("running"))
            .add(
                Condition::any()
                    .add(runner_manager::Column::Enabled.ne(true))
                    .add(runner_manager::Column::Status.ne("online"))
                    .add(
                        Expr::col(runner_manager::Column::LastSeenAt).lte(Expr::cust_with_values(
                            "NOW() - ($1::double precision * INTERVAL '1 second')",
                            [ttl],
                        )),
                    )
                    .add(Expr::col(workspace_runner::Column::LastObservedAt).lte(
                        Expr::cust_with_values(
                            "NOW() - ($1::double precision * INTERVAL '1 second')",
                            [ttl * 2],
                        ),
                    )),
            ),
        Expr::val("stale"),
    )
    .finally(workspace_runner::Column::Status.into_expr())
    .into()
}

fn list_query(params: &ListWorkspaceRunnersParams) -> sea_orm::Select<workspace_runner::Entity> {
    let mut query = workspace_runner::Entity::find().left_join(runner_manager::Entity);
    if let Some(status) = params.status.as_deref() {
        query = query.filter(status_expr().eq(status));
    } else {
        query = query.filter(workspace_runner::Column::Status.ne("removed"));
    }
    if let Some(owner_kind) = params.owner_kind.as_deref() {
        query = query.filter(workspace_runner::Column::OwnerKind.eq(owner_kind));
    }
    query
}

#[derive(FromQueryResult)]
struct ListRow {
    runner_id: i64,
    runner_manager_id: Option<i64>,
    owner_kind: String,
    owner_user_id: Option<i64>,
    owner_workspace_binding_id: Option<i64>,
    owner_application_id: Option<i64>,
    owner_principal_digest: Option<String>,
    container_name: String,
    container_id: Option<String>,
    runtime: String,
    runtime_class: Option<String>,
    image_name: String,
    status: String,
    network_enabled: bool,
    workspace_root: String,
    last_active_at: DateTime<Utc>,
    last_observed_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_error: Option<String>,
}

impl From<ListRow> for WorkspaceRunnerRecord {
    fn from(row: ListRow) -> Self {
        Self {
            runner_id: row.runner_id,
            runner_manager_id: row.runner_manager_id,
            owner_kind: row.owner_kind,
            owner_user_id: row.owner_user_id,
            owner_workspace_binding_id: row.owner_workspace_binding_id,
            owner_application_id: row.owner_application_id,
            owner_principal_digest: row.owner_principal_digest,
            container_name: row.container_name,
            container_id: row.container_id,
            runtime: row.runtime,
            runtime_class: row.runtime_class,
            image_name: row.image_name,
            status: row.status,
            network_enabled: row.network_enabled,
            workspace_root: row.workspace_root,
            last_active_at: row.last_active_at,
            last_observed_at: row.last_observed_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_error: row.last_error,
        }
    }
}

pub async fn list_workspace_runners(
    pool: &PgPool,
    params: &ListWorkspaceRunnersParams,
) -> anyhow::Result<Page<WorkspaceRunnerRecord>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let db = native(pool);
    let total = list_query(params).count(&db).await? as i64;
    let items = list_query(params)
        .select_only()
        .columns([
            workspace_runner::Column::RunnerId,
            workspace_runner::Column::RunnerManagerId,
            workspace_runner::Column::OwnerKind,
            workspace_runner::Column::OwnerUserId,
            workspace_runner::Column::OwnerWorkspaceBindingId,
            workspace_runner::Column::OwnerApplicationId,
            workspace_runner::Column::OwnerPrincipalDigest,
            workspace_runner::Column::ContainerName,
            workspace_runner::Column::ContainerId,
            workspace_runner::Column::Runtime,
            workspace_runner::Column::RuntimeClass,
            workspace_runner::Column::ImageName,
            workspace_runner::Column::NetworkEnabled,
            workspace_runner::Column::WorkspaceRoot,
            workspace_runner::Column::LastActiveAt,
            workspace_runner::Column::LastObservedAt,
            workspace_runner::Column::CreatedAt,
            workspace_runner::Column::UpdatedAt,
            workspace_runner::Column::LastError,
        ])
        .expr_as(status_expr(), "status")
        .order_by(workspace_runner::Column::RunnerId, sea_orm::Order::Asc)
        .limit(limit as u64)
        .offset(offset as u64)
        .into_model::<ListRow>()
        .all(&db)
        .await?
        .into_iter()
        .map(WorkspaceRunnerRecord::from)
        .collect();
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

// Cross-domain aggregate subqueries are clearer and safer as one static SQL
// statement than as a composition of the entity builders.
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

#[derive(FromQueryResult)]
struct OwnerRow {
    owner_application_id: Option<i64>,
    owner_workspace_binding_id: Option<i64>,
    owner_principal_digest: Option<String>,
}

pub async fn list_workspace_runner_owners_by_binding(
    pool: &PgPool,
    binding_id: i64,
) -> anyhow::Result<Vec<RunnerOwner>> {
    let rows = workspace_runner::Entity::find()
        .select_only()
        .columns([
            workspace_runner::Column::OwnerApplicationId,
            workspace_runner::Column::OwnerWorkspaceBindingId,
            workspace_runner::Column::OwnerPrincipalDigest,
        ])
        .filter(workspace_runner::Column::OwnerKind.eq("application_principal"))
        .filter(workspace_runner::Column::OwnerWorkspaceBindingId.eq(binding_id))
        .order_by(
            workspace_runner::Column::OwnerApplicationId,
            sea_orm::Order::Asc,
        )
        .order_by(
            workspace_runner::Column::OwnerWorkspaceBindingId,
            sea_orm::Order::Asc,
        )
        .order_by(
            workspace_runner::Column::OwnerPrincipalDigest,
            sea_orm::Order::Asc,
        )
        .distinct()
        .into_model::<OwnerRow>()
        .all(&native(pool))
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RunnerOwner::ApplicationPrincipal {
                application_id: row.owner_application_id.ok_or_else(|| {
                    anyhow::anyhow!("workspace runner owner row has no application id")
                })?,
                workspace_binding_id: row.owner_workspace_binding_id.ok_or_else(|| {
                    anyhow::anyhow!("workspace runner owner row has no binding id")
                })?,
                principal_digest: row.owner_principal_digest.ok_or_else(|| {
                    anyhow::anyhow!("workspace runner owner row has no principal digest")
                })?,
            })
        })
        .collect()
}

// The conditional owner-scoped upsert preserves atomic single-statement
// conflict behavior; it remains static SQL rather than a hand-built query.
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

    sqlx::query_file_as!(
        WorkspaceRunnerRecord,
        "src/sql/save_workspace_runner.sql",
        input.runner_manager_id,
        owner_kind,
        owner_user_id,
        owner_workspace_binding_id,
        owner_application_id,
        owner_principal_digest,
        input.container_name,
        input.container_id,
        input.runtime,
        input.runtime_class,
        input.image_name,
        input.status,
        input.network_enabled,
        input.workspace_root,
        input.last_error,
    )
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// Owner-kind-specific lifecycle reporting stays SQLx so the conditional
// single-statement update remains atomic and readable.
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

    sqlx::query_file!(
        "src/sql/report_workspace_runner.sql",
        runner_manager_id,
        &event.container_name,
        owner_kind,
        status,
        event.container_id.as_deref(),
        event.last_error.as_deref(),
        owner_user_id,
        owner_workspace_binding_id,
        owner_application_id,
        owner_principal_digest,
    )
    .execute(pool)
    .await?;
    Ok(())
}
