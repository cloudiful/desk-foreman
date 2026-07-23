use sqlx::PgPool;

use super::types::{AuditLogPageResponse, AuditLogResponse, ListAuditLogsParams};

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

pub async fn record_audit(pool: &PgPool, entry: AuditLogEntry<'_>) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/insert_audit_log.sql"))
        .bind(entry.actor_user_id)
        .bind(entry.actor_application_id)
        .bind(entry.actor_type)
        .bind(entry.action)
        .bind(entry.target_type)
        .bind(entry.target_id)
        .bind(entry.workspace_binding_id)
        .bind(entry.external_user_id)
        .bind(entry.payload)
        .bind(entry.request_id)
        .bind(entry.session_id)
        .bind(entry.duration_ms)
        .bind(entry.status)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_audit_logs(
    pool: &PgPool,
    params: &ListAuditLogsParams,
) -> anyhow::Result<AuditLogPageResponse> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_audit_logs.sql"))
        .bind(params.from)
        .bind(params.to)
        .bind(&params.action)
        .bind(params.actor_user_id)
        .bind(params.actor_application_id)
        .bind(params.workspace_binding_id)
        .bind(params.session_id)
        .bind(&params.request_id)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, AuditLogResponse>(include_str!("../sql/list_audit_logs.sql"))
        .bind(params.from)
        .bind(params.to)
        .bind(&params.action)
        .bind(params.actor_user_id)
        .bind(params.actor_application_id)
        .bind(params.workspace_binding_id)
        .bind(params.session_id)
        .bind(&params.request_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(AuditLogPageResponse {
        items,
        total,
        limit,
        offset,
    })
}
