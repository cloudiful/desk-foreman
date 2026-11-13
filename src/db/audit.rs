use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Select, sea_query::ExprTrait,
};
use sqlx::PgPool;

use super::native::audit_log;
use super::types::{AuditLogResponse, ListAuditLogsParams, Page};

pub struct AuditLogEntry<'a> {
    pub actor_user_id: Option<i64>,
    pub actor_application_id: Option<i64>,
    pub actor_type: &'a str,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: &'a str,
    pub workspace_binding_id: Option<i64>,
    pub external_user_id: Option<&'a str>,
    pub payload: serde_json::Value,
    pub request_id: Option<&'a str>,
    pub session_id: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: Option<&'a str>,
}

fn native(pool: &PgPool) -> DatabaseConnection {
    DatabaseConnection::from(pool.clone())
}

fn response(model: audit_log::Model) -> AuditLogResponse {
    AuditLogResponse {
        audit_id: model.audit_id,
        actor_user_id: model.actor_user_id,
        actor_application_id: model.actor_application_id,
        actor_type: model.actor_type,
        action: model.action,
        target_type: model.target_type,
        target_id: model.target_id,
        workspace_binding_id: model.workspace_binding_id,
        external_user_id: model.external_user_id,
        payload: model.payload,
        request_id: model.request_id,
        session_id: model.session_id,
        duration_ms: model.duration_ms,
        status: model.status,
        created_at: model.created_at,
    }
}

fn audit_query(params: &ListAuditLogsParams) -> Select<audit_log::Entity> {
    let mut query = audit_log::Entity::find();
    if let Some(from) = params.from {
        query = query.filter(audit_log::Column::CreatedAt.gte(from));
    }
    if let Some(to) = params.to {
        query = query.filter(audit_log::Column::CreatedAt.lt(to));
    }
    if let Some(action) = params.action.as_deref() {
        query = query.filter(audit_log::Column::Action.eq(action));
    }
    if let Some(search) = params.search.as_deref() {
        let pattern = format!("%{search}%");
        query = query.filter(
            Condition::any()
                .add(audit_log::Column::Action.ilike(pattern.clone()))
                .add(audit_log::Column::ActorType.ilike(pattern.clone()))
                .add(audit_log::Column::TargetType.ilike(pattern)),
        );
    }
    if let Some(status) = params.status.as_deref() {
        let filter = match status {
            "success" => audit_log::Column::Status.eq("success"),
            "failure" => audit_log::Column::Status
                .is_not_null()
                .and(audit_log::Column::Status.ne("success")),
            _ => audit_log::Column::Status
                .is_null()
                .or(audit_log::Column::Status.eq("unknown")),
        };
        query = query.filter(filter);
    }
    if let Some(actor_user_id) = params.actor_user_id {
        query = query.filter(audit_log::Column::ActorUserId.eq(actor_user_id));
    }
    if let Some(actor_application_id) = params.actor_application_id {
        query = query.filter(audit_log::Column::ActorApplicationId.eq(actor_application_id));
    }
    if let Some(workspace_binding_id) = params.workspace_binding_id {
        query = query.filter(audit_log::Column::WorkspaceBindingId.eq(workspace_binding_id));
    }
    if let Some(session_id) = params.session_id {
        query = query.filter(audit_log::Column::SessionId.eq(session_id));
    }
    if let Some(request_id) = params.request_id.as_deref() {
        query = query.filter(audit_log::Column::RequestId.eq(request_id));
    }
    query
}

pub async fn record_audit(pool: &PgPool, entry: AuditLogEntry<'_>) -> anyhow::Result<()> {
    audit_log::Entity::insert(audit_log::ActiveModel {
        actor_user_id: sea_orm::Set(entry.actor_user_id),
        actor_application_id: sea_orm::Set(entry.actor_application_id),
        actor_type: sea_orm::Set(entry.actor_type.to_owned()),
        action: sea_orm::Set(entry.action.to_owned()),
        target_type: sea_orm::Set(entry.target_type.to_owned()),
        target_id: sea_orm::Set(entry.target_id.to_owned()),
        workspace_binding_id: sea_orm::Set(entry.workspace_binding_id),
        external_user_id: sea_orm::Set(entry.external_user_id.map(str::to_owned)),
        payload: sea_orm::Set(entry.payload),
        request_id: sea_orm::Set(entry.request_id.map(str::to_owned)),
        session_id: sea_orm::Set(entry.session_id),
        duration_ms: sea_orm::Set(entry.duration_ms),
        status: sea_orm::Set(entry.status.map(str::to_owned)),
        ..Default::default()
    })
    .exec_without_returning(&native(pool))
    .await?;
    Ok(())
}

pub async fn list_audit_logs(
    pool: &PgPool,
    params: &ListAuditLogsParams,
) -> anyhow::Result<Page<AuditLogResponse>> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let db = native(pool);
    let total = audit_query(params).count(&db).await? as i64;
    let items = audit_query(params)
        .order_by(audit_log::Column::CreatedAt, sea_orm::Order::Desc)
        .order_by(audit_log::Column::AuditId, sea_orm::Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(&db)
        .await?
        .into_iter()
        .map(response)
        .collect();
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}
