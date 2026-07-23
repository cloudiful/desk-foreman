use sqlx::PgPool;

use super::types::{ApprovalSettingsRecord, UpdateApprovalSettingsRequest};

pub async fn get_approval_settings(pool: &PgPool) -> anyhow::Result<ApprovalSettingsRecord> {
    sqlx::query_as::<_, ApprovalSettingsRecord>(include_str!("../sql/get_approval_settings.sql"))
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn update_approval_settings(
    pool: &PgPool,
    request: &UpdateApprovalSettingsRequest,
) -> anyhow::Result<ApprovalSettingsRecord> {
    sqlx::query_as::<_, ApprovalSettingsRecord>(include_str!("../sql/update_approval_settings.sql"))
        .bind(&request.endpoint)
        .bind(&request.model)
        .bind(request.timeout_ms)
        .bind(request.max_input_bytes)
        .bind(request.max_concurrent)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}
