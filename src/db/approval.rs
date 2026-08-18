use sqlx::PgPool;

use super::types::{
    ApplicationApprovalSecretRecord, ApprovalSettingsRecord, UpdateApprovalSettingsRequest,
};
use crate::secrets::EncryptedSecret;

pub async fn get_approval_settings(pool: &PgPool) -> anyhow::Result<ApprovalSettingsRecord> {
    sqlx::query_as::<_, ApprovalSettingsRecord>(include_str!("../sql/get_approval_settings.sql"))
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn update_approval_settings(
    pool: &PgPool,
    request: &UpdateApprovalSettingsRequest,
    secret: Option<&EncryptedSecret>,
) -> anyhow::Result<ApprovalSettingsRecord> {
    sqlx::query_as::<_, ApprovalSettingsRecord>(include_str!("../sql/update_approval_settings.sql"))
        .bind(request.enabled)
        .bind(&request.endpoint)
        .bind(&request.model)
        .bind(request.timeout_ms)
        .bind(request.max_input_bytes)
        .bind(request.max_concurrent)
        .bind(secret.map(|value| value.ciphertext.clone()))
        .bind(secret.map(|value| value.nonce.clone()))
        .bind(secret.map(|value| value.key_version))
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn get_application_approval_secret(
    pool: &PgPool,
    application_id: i64,
) -> anyhow::Result<Option<ApplicationApprovalSecretRecord>> {
    sqlx::query_as::<_, ApplicationApprovalSecretRecord>(include_str!(
        "../sql/get_application_approval_secret.sql"
    ))
    .bind(application_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}
