use runner_protocol::{
    RUNNER_MANAGER_HEARTBEAT_TTL_SECS, RUNNER_MANAGER_HEARTBEAT_WRITE_INTERVAL_SECS,
};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, ExprTrait, NullOrdering},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    auth::hash_bearer_token,
    db::native::runner_manager,
    db::types::{
        CreateRunnerManagerRequest, ListRunnerManagersParams, Page, RunnerManagerAuthRecord,
        RunnerManagerRecord, RunnerManagerResponse, UpdateRunnerManagerRequest,
    },
};

fn native(pool: &PgPool) -> DatabaseConnection {
    DatabaseConnection::from(pool.clone())
}

fn response(model: runner_manager::Model) -> RunnerManagerResponse {
    RunnerManagerResponse {
        runner_manager_id: model.runner_manager_id,
        name: model.name,
        endpoint: model.endpoint,
        enabled: model.enabled,
        image: model.image,
        network_enabled: model.network_enabled,
        max_output_bytes: model.max_output_bytes,
        max_timeout_ms: model.max_timeout_ms,
        max_sessions: model.max_sessions,
        pids_limit: model.pids_limit,
        memory_limit: model.memory_limit,
        cpu_limit: model.cpu_limit,
        host_workspace_root: model.host_workspace_root,
        status: model.status,
        last_seen_at: model.last_seen_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn record(model: runner_manager::Model) -> RunnerManagerRecord {
    RunnerManagerRecord {
        runner_manager_id: model.runner_manager_id,
        name: model.name,
        endpoint: model.endpoint,
        access_token_hash: model.access_token_hash,
        enabled: model.enabled,
        image: model.image,
        network_enabled: model.network_enabled,
        max_output_bytes: model.max_output_bytes,
        max_timeout_ms: model.max_timeout_ms,
        max_sessions: model.max_sessions,
        pids_limit: model.pids_limit,
        memory_limit: model.memory_limit,
        cpu_limit: model.cpu_limit,
        host_workspace_root: model.host_workspace_root,
        status: model.status,
        last_seen_at: model.last_seen_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn live_manager_condition() -> Condition {
    Condition::all()
        .add(runner_manager::Column::Enabled.eq(true))
        .add(runner_manager::Column::Status.eq("online"))
        .add(
            Expr::col(runner_manager::Column::LastSeenAt).gt(Expr::cust_with_values(
                "NOW() - ($1::double precision * INTERVAL '1 second')",
                [RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64],
            )),
        )
}

fn displayed_status_expr() -> Expr {
    Expr::case(
        live_manager_condition(),
        runner_manager::Column::Status.into_expr(),
    )
    .finally(Expr::val("offline"))
    .into()
}

#[derive(FromQueryResult)]
struct ListRow {
    runner_manager_id: i64,
    name: String,
    endpoint: String,
    enabled: bool,
    image: String,
    network_enabled: bool,
    max_output_bytes: i64,
    max_timeout_ms: i64,
    max_sessions: i64,
    pids_limit: i64,
    memory_limit: String,
    cpu_limit: String,
    host_workspace_root: Option<String>,
    status: String,
    last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ListRow> for RunnerManagerResponse {
    fn from(row: ListRow) -> Self {
        Self {
            runner_manager_id: row.runner_manager_id,
            name: row.name,
            endpoint: row.endpoint,
            enabled: row.enabled,
            image: row.image,
            network_enabled: row.network_enabled,
            max_output_bytes: row.max_output_bytes,
            max_timeout_ms: row.max_timeout_ms,
            max_sessions: row.max_sessions,
            pids_limit: row.pids_limit,
            memory_limit: row.memory_limit,
            cpu_limit: row.cpu_limit,
            host_workspace_root: row.host_workspace_root,
            status: row.status,
            last_seen_at: row.last_seen_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromQueryResult)]
struct AuthRow {
    runner_manager_id: i64,
    name: String,
    endpoint: String,
    access_token_hash: String,
    enabled: bool,
    image: String,
    network_enabled: bool,
    max_output_bytes: i64,
    max_timeout_ms: i64,
    max_sessions: i64,
    pids_limit: i64,
    memory_limit: String,
    cpu_limit: String,
    host_workspace_root: Option<String>,
    status: String,
    last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    heartbeat_due: bool,
}

impl AuthRow {
    fn into_auth(self) -> RunnerManagerAuthRecord {
        RunnerManagerAuthRecord {
            manager: RunnerManagerRecord {
                runner_manager_id: self.runner_manager_id,
                name: self.name,
                endpoint: self.endpoint,
                access_token_hash: self.access_token_hash,
                enabled: self.enabled,
                image: self.image,
                network_enabled: self.network_enabled,
                max_output_bytes: self.max_output_bytes,
                max_timeout_ms: self.max_timeout_ms,
                max_sessions: self.max_sessions,
                pids_limit: self.pids_limit,
                memory_limit: self.memory_limit,
                cpu_limit: self.cpu_limit,
                host_workspace_root: self.host_workspace_root,
                status: self.status,
                last_seen_at: self.last_seen_at,
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
            heartbeat_due: self.heartbeat_due,
        }
    }
}

fn list_query(params: &ListRunnerManagersParams) -> sea_orm::Select<runner_manager::Entity> {
    let mut query = runner_manager::Entity::find();
    if let Some(search) = params.search.as_deref() {
        query = query.filter(runner_manager::Column::Name.ilike(format!("%{search}%")));
    }
    if let Some(enabled) = params.enabled {
        query = query.filter(runner_manager::Column::Enabled.eq(enabled));
    }
    query
}

pub async fn list_runner_managers(
    pool: &PgPool,
    params: &ListRunnerManagersParams,
) -> anyhow::Result<Page<RunnerManagerResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let db = native(pool);
    let total = list_query(params).count(&db).await? as i64;
    let items = list_query(params)
        .select_only()
        .columns([
            runner_manager::Column::RunnerManagerId,
            runner_manager::Column::Name,
            runner_manager::Column::Endpoint,
            runner_manager::Column::Enabled,
            runner_manager::Column::Image,
            runner_manager::Column::NetworkEnabled,
            runner_manager::Column::MaxOutputBytes,
            runner_manager::Column::MaxTimeoutMs,
            runner_manager::Column::MaxSessions,
            runner_manager::Column::PidsLimit,
            runner_manager::Column::MemoryLimit,
            runner_manager::Column::CpuLimit,
            runner_manager::Column::HostWorkspaceRoot,
            runner_manager::Column::LastSeenAt,
            runner_manager::Column::CreatedAt,
            runner_manager::Column::UpdatedAt,
        ])
        .expr_as(displayed_status_expr(), "status")
        .order_by(runner_manager::Column::RunnerManagerId, sea_orm::Order::Asc)
        .limit(limit as u64)
        .offset(offset as u64)
        .into_model::<ListRow>()
        .all(&db)
        .await?
        .into_iter()
        .map(RunnerManagerResponse::from)
        .collect();
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn create_runner_manager(
    pool: &PgPool,
    request: &CreateRunnerManagerRequest,
) -> anyhow::Result<(RunnerManagerResponse, Option<String>)> {
    let generated = request
        .access_token
        .as_deref()
        .is_none_or(|value| value.trim().is_empty());
    let token = request
        .access_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("dfm_{}", Uuid::new_v4().simple()));
    let model = runner_manager::Entity::insert(runner_manager::ActiveModel {
        name: sea_orm::Set(request.name.clone()),
        endpoint: sea_orm::Set(request.endpoint.clone()),
        access_token_hash: sea_orm::Set(hash_bearer_token(&token)),
        image: sea_orm::Set(request.image.clone()),
        network_enabled: sea_orm::Set(request.network_enabled),
        max_output_bytes: sea_orm::Set(request.max_output_bytes),
        max_timeout_ms: sea_orm::Set(request.max_timeout_ms),
        max_sessions: sea_orm::Set(request.max_sessions),
        pids_limit: sea_orm::Set(request.pids_limit),
        memory_limit: sea_orm::Set(request.memory_limit.clone()),
        cpu_limit: sea_orm::Set(request.cpu_limit.clone()),
        host_workspace_root: sea_orm::Set(request.host_workspace_root.clone()),
        ..Default::default()
    })
    .exec_with_returning(&native(pool))
    .await?;
    Ok((response(model), generated.then_some(token)))
}

pub async fn update_runner_manager(
    pool: &PgPool,
    manager_id: i64,
    request: &UpdateRunnerManagerRequest,
) -> anyhow::Result<Option<RunnerManagerResponse>> {
    let model = runner_manager::Entity::update_many()
        .col_expr(
            runner_manager::Column::Endpoint,
            Expr::val(request.endpoint.clone()),
        )
        .col_expr(runner_manager::Column::Enabled, Expr::val(request.enabled))
        .col_expr(
            runner_manager::Column::Image,
            Expr::val(request.image.clone()),
        )
        .col_expr(
            runner_manager::Column::NetworkEnabled,
            Expr::val(request.network_enabled),
        )
        .col_expr(
            runner_manager::Column::MaxOutputBytes,
            Expr::val(request.max_output_bytes),
        )
        .col_expr(
            runner_manager::Column::MaxTimeoutMs,
            Expr::val(request.max_timeout_ms),
        )
        .col_expr(
            runner_manager::Column::MaxSessions,
            Expr::val(request.max_sessions),
        )
        .col_expr(
            runner_manager::Column::PidsLimit,
            Expr::val(request.pids_limit),
        )
        .col_expr(
            runner_manager::Column::MemoryLimit,
            Expr::val(request.memory_limit.clone()),
        )
        .col_expr(
            runner_manager::Column::CpuLimit,
            Expr::val(request.cpu_limit.clone()),
        )
        .col_expr(
            runner_manager::Column::HostWorkspaceRoot,
            Expr::val(request.host_workspace_root.clone()),
        )
        .col_expr(runner_manager::Column::UpdatedAt, Expr::cust("NOW()"))
        .filter(runner_manager::Column::RunnerManagerId.eq(manager_id))
        .exec_with_returning(&native(pool))
        .await?
        .into_iter()
        .next();
    Ok(model.map(response))
}

pub async fn find_runner_manager_auth_by_token(
    pool: &PgPool,
    token: &str,
) -> anyhow::Result<Option<RunnerManagerAuthRecord>> {
    let heartbeat_due = Condition::any()
        .add(runner_manager::Column::Status.ne("online"))
        .add(runner_manager::Column::LastSeenAt.is_null())
        .add(
            Expr::col(runner_manager::Column::LastSeenAt).lte(Expr::cust_with_values(
                "NOW() - ($1::double precision * INTERVAL '1 second')",
                [RUNNER_MANAGER_HEARTBEAT_WRITE_INTERVAL_SECS as i64],
            )),
        );
    Ok(runner_manager::Entity::find()
        .select_only()
        .columns([
            runner_manager::Column::RunnerManagerId,
            runner_manager::Column::Name,
            runner_manager::Column::Endpoint,
            runner_manager::Column::AccessTokenHash,
            runner_manager::Column::Enabled,
            runner_manager::Column::Image,
            runner_manager::Column::NetworkEnabled,
            runner_manager::Column::MaxOutputBytes,
            runner_manager::Column::MaxTimeoutMs,
            runner_manager::Column::MaxSessions,
            runner_manager::Column::PidsLimit,
            runner_manager::Column::MemoryLimit,
            runner_manager::Column::CpuLimit,
            runner_manager::Column::HostWorkspaceRoot,
            runner_manager::Column::Status,
            runner_manager::Column::LastSeenAt,
            runner_manager::Column::CreatedAt,
            runner_manager::Column::UpdatedAt,
        ])
        .expr_as(
            Expr::case(heartbeat_due, Expr::val(true)).finally(Expr::val(false)),
            "heartbeat_due",
        )
        .filter(runner_manager::Column::AccessTokenHash.eq(hash_bearer_token(token)))
        .into_model::<AuthRow>()
        .one(&native(pool))
        .await?
        .map(AuthRow::into_auth))
}

pub async fn find_runner_manager_by_token(
    pool: &PgPool,
    token: &str,
) -> anyhow::Result<Option<RunnerManagerRecord>> {
    Ok(find_runner_manager_auth_by_token(pool, token)
        .await?
        .map(|auth| auth.manager))
}

pub async fn find_enabled_runner_manager(
    pool: &PgPool,
) -> anyhow::Result<Option<RunnerManagerRecord>> {
    Ok(runner_manager::Entity::find()
        .filter(runner_manager::Column::Enabled.eq(true))
        .filter(runner_manager::Column::Status.eq("online"))
        .filter(
            Expr::col(runner_manager::Column::LastSeenAt).gt(Expr::cust_with_values(
                "NOW() - ($1::double precision * INTERVAL '1 second')",
                [RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64],
            )),
        )
        .order_by_with_nulls(
            runner_manager::Column::LastSeenAt,
            sea_orm::Order::Desc,
            NullOrdering::Last,
        )
        .order_by(runner_manager::Column::RunnerManagerId, sea_orm::Order::Asc)
        .one(&native(pool))
        .await?
        .map(record))
}

pub async fn list_registered_runner_manager_ids(pool: &PgPool) -> anyhow::Result<Vec<i64>> {
    Ok(runner_manager::Entity::find()
        .select_only()
        .column(runner_manager::Column::RunnerManagerId)
        .order_by(runner_manager::Column::RunnerManagerId, sea_orm::Order::Asc)
        .into_tuple::<i64>()
        .all(&native(pool))
        .await?)
}

pub async fn list_live_runner_manager_ids(pool: &PgPool) -> anyhow::Result<Vec<i64>> {
    Ok(runner_manager::Entity::find()
        .select_only()
        .column(runner_manager::Column::RunnerManagerId)
        .filter(runner_manager::Column::Enabled.eq(true))
        .filter(runner_manager::Column::Status.eq("online"))
        .filter(
            Expr::col(runner_manager::Column::LastSeenAt).gt(Expr::cust_with_values(
                "NOW() - ($1::double precision * INTERVAL '1 second')",
                [RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64],
            )),
        )
        .into_tuple::<i64>()
        .all(&native(pool))
        .await?)
}

// The guarded heartbeat refresh is concurrency-sensitive and observed by the
// heartbeat regression suite through its exact single-statement SQL text, so it
// keeps the static SQL implementation.
pub async fn touch_runner_manager(pool: &PgPool, manager_id: i64) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_runner_manager.sql"))
        .bind(manager_id)
        .bind(RUNNER_MANAGER_HEARTBEAT_WRITE_INTERVAL_SECS as i64)
        .execute(pool)
        .await?;
    Ok(())
}
