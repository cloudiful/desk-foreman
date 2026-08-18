use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::bail;
use desk_foreman_approval::{
    ApprovalError, ApprovalMode, ApprovalReviewer, OpenAiReviewer, OpenAiReviewerConfig,
    ReviewDecision, ReviewRequest,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::{
    AppState,
    actor::ActorContext,
    db::types::{ApplicationResponse, ApprovalSettingsRecord},
    secrets::{EncryptedSecret, SecretManager},
};

type Reviewer = Arc<dyn ApprovalReviewer>;

#[derive(Clone)]
pub struct ApprovalService {
    env_api_key: Option<String>,
    secret_manager: Option<SecretManager>,
    cache: Arc<RwLock<HashMap<String, Reviewer>>>,
    database_backed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct EffectiveConfig {
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) api_key: String,
    pub(crate) timeout_ms: u64,
    pub(crate) max_input_bytes: usize,
    pub(crate) max_concurrent: usize,
    pub(crate) max_output_tokens: u32,
}

#[derive(Clone, Debug)]
pub struct ApiKeyStatus {
    pub configured: bool,
    pub source: &'static str,
    pub secret_storage_ready: bool,
}

#[derive(Debug, Error)]
pub enum ApprovalTestError {
    #[error("approval reviewer is disabled or not configured")]
    DisabledOrNotConfigured,
    #[error("approval reviewer API key is not configured")]
    ApiKeyMissing,
    #[error("approval reviewer secret storage is not configured")]
    SecretStorage,
    #[error("approval reviewer configuration is invalid")]
    InvalidConfiguration,
    #[error(transparent)]
    Provider(#[from] ApprovalError),
    #[error("approval reviewer configuration lookup failed")]
    Database(#[source] anyhow::Error),
}

impl ApprovalService {
    pub async fn from_env_or_database(pool: &sqlx::PgPool) -> anyhow::Result<Self> {
        let seed = std::env::var(crate::secrets::MASTER_KEY_ENV).ok();
        let master_secret =
            crate::db::secrets::get_or_create_master_secret(pool, seed.as_deref()).await?;
        let secret_manager = SecretManager::from_master_secret(&master_secret)?;
        Ok(Self {
            env_api_key: std::env::var("APPROVAL_API_KEY")
                .ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            secret_manager: Some(secret_manager),
            cache: Arc::new(RwLock::new(HashMap::new())),
            database_backed: true,
        })
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            env_api_key: None,
            secret_manager: None,
            cache: Arc::new(RwLock::new(HashMap::new())),
            database_backed: false,
        }
    }

    pub fn encrypt_api_key(&self, api_key: &str) -> anyhow::Result<EncryptedSecret> {
        self.secret_manager
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!("{0} is not configured", crate::secrets::MASTER_KEY_ENV)
            })?
            .encrypt(api_key)
    }

    pub fn decrypt_api_key(&self, secret: &EncryptedSecret) -> anyhow::Result<String> {
        self.secret_manager
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!("{0} is not configured", crate::secrets::MASTER_KEY_ENV)
            })?
            .decrypt(secret)
    }

    pub fn global_api_key_status(&self, settings: &ApprovalSettingsRecord) -> ApiKeyStatus {
        if settings.api_key_ciphertext.is_some()
            || settings.api_key_nonce.is_some()
            || settings.api_key_key_version.is_some()
        {
            let envelope_is_complete = encrypted_secret_from_parts(
                settings.api_key_ciphertext.clone(),
                settings.api_key_nonce.clone(),
                settings.api_key_key_version,
            )
            .is_ok_and(|secret| secret.is_some());
            return ApiKeyStatus {
                configured: true,
                source: "database",
                secret_storage_ready: self.secret_manager.is_some() && envelope_is_complete,
            };
        }
        let configured = self
            .env_api_key
            .as_deref()
            .is_some_and(|value| !value.is_empty());
        ApiKeyStatus {
            configured,
            source: if configured { "environment" } else { "none" },
            secret_storage_ready: self.secret_manager.is_some(),
        }
    }

    pub async fn reviewer_for_actor(
        &self,
        state: &AppState,
        actor: &ActorContext,
    ) -> anyhow::Result<Option<Reviewer>> {
        if !self.database_backed {
            return Ok(None);
        }
        let Some(config) = self
            .effective_config(state, actor.application.as_ref())
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(self.reviewer_for_config(config).await?))
    }

    pub async fn test_global(&self, state: &AppState) -> Result<ReviewDecision, ApprovalTestError> {
        let settings = crate::db::queries::get_approval_settings(&state.db)
            .await
            .map_err(ApprovalTestError::Database)?;
        let Some(config) = self
            .global_config(&settings)
            .await
            .map_err(map_test_config_error)?
        else {
            return Err(ApprovalTestError::DisabledOrNotConfigured);
        };
        self.test_config(config).await
    }

    pub async fn test_application(
        &self,
        state: &AppState,
        application: &ApplicationResponse,
    ) -> Result<ReviewDecision, ApprovalTestError> {
        let settings = crate::db::queries::get_approval_settings(&state.db)
            .await
            .map_err(ApprovalTestError::Database)?;
        let config = match ApprovalMode::parse(&application.approval_mode) {
            Some(ApprovalMode::Disabled) => {
                return Err(ApprovalTestError::DisabledOrNotConfigured);
            }
            Some(ApprovalMode::Enabled) => self
                .application_config(state, &settings, application)
                .await
                .map_err(map_test_config_error)?,
            Some(ApprovalMode::Inherit) => self
                .global_config(&settings)
                .await
                .map_err(map_test_config_error)?,
            None => return Err(ApprovalTestError::InvalidConfiguration),
        };
        let Some(config) = config else {
            return Err(ApprovalTestError::DisabledOrNotConfigured);
        };
        self.test_config(config).await
    }

    async fn test_config(
        &self,
        config: EffectiveConfig,
    ) -> Result<ReviewDecision, ApprovalTestError> {
        let reviewer = self
            .reviewer_for_config(config)
            .await
            .map_err(map_test_config_error)?;
        reviewer
            .review(&ReviewRequest::shell(
                "printf 'desk-foreman approval reviewer connectivity test'",
                None,
                serde_json::json!({
                    "synthetic": true,
                    "workspace_scoped": true,
                    "purpose": "connectivity_test",
                }),
            ))
            .await
            .map_err(ApprovalTestError::Provider)
    }

    async fn reviewer_for_config(&self, config: EffectiveConfig) -> anyhow::Result<Reviewer> {
        if config.api_key.trim().is_empty() {
            bail!("approval reviewer API key is not configured");
        }
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            config.endpoint,
            config.model,
            config.timeout_ms,
            config.max_input_bytes,
            config.max_concurrent,
            config.max_output_tokens,
            digest(&config.api_key),
        );
        if let Some(reviewer) = self.cache.read().await.get(&key).cloned() {
            return Ok(reviewer);
        }
        let reviewer = Arc::new(OpenAiReviewer::new(OpenAiReviewerConfig {
            api_base: config.endpoint,
            api_key: Some(config.api_key),
            model: config.model,
            timeout: Duration::from_millis(config.timeout_ms),
            max_input_bytes: config.max_input_bytes,
            max_concurrent: config.max_concurrent,
            max_output_tokens: config.max_output_tokens,
        })?) as Reviewer;
        self.cache.write().await.insert(key, reviewer.clone());
        Ok(reviewer)
    }

    async fn effective_config(
        &self,
        state: &AppState,
        application: Option<&ApplicationResponse>,
    ) -> anyhow::Result<Option<EffectiveConfig>> {
        let global = crate::db::queries::get_approval_settings(&state.db).await?;
        match application {
            Some(application) => match ApprovalMode::parse(&application.approval_mode) {
                Some(ApprovalMode::Disabled) => Ok(None),
                Some(ApprovalMode::Enabled) => {
                    self.application_config(state, &global, application).await
                }
                Some(ApprovalMode::Inherit) => self.global_config(&global).await,
                None => bail!("invalid application approval mode"),
            },
            None => self.global_config(&global).await,
        }
    }

    async fn global_config(
        &self,
        global: &ApprovalSettingsRecord,
    ) -> anyhow::Result<Option<EffectiveConfig>> {
        if !global.enabled {
            return Ok(None);
        }
        let api_key = self.global_api_key(global)?;
        build_config(
            global.endpoint.clone().unwrap_or_default(),
            global.model.clone().unwrap_or_default(),
            api_key,
            global.timeout_ms,
            global.max_input_bytes,
            global.max_concurrent,
            global.max_output_tokens,
            false,
        )
    }

    async fn application_config(
        &self,
        state: &AppState,
        global: &ApprovalSettingsRecord,
        application: &ApplicationResponse,
    ) -> anyhow::Result<Option<EffectiveConfig>> {
        let secret = crate::db::queries::get_application_approval_secret(
            &state.db,
            application.application_id,
        )
        .await?;
        let secret = secret
            .map(|secret| {
                encrypted_secret_from_parts(
                    secret.api_key_ciphertext,
                    secret.api_key_nonce,
                    secret.api_key_key_version,
                )
            })
            .transpose()?
            .flatten();
        let api_key = secret
            .map(|secret| self.decrypt_api_key(&secret))
            .transpose()?;
        build_config(
            application.approval_endpoint.clone().unwrap_or_default(),
            application.approval_model.clone().unwrap_or_default(),
            api_key,
            application.approval_timeout_ms.unwrap_or(global.timeout_ms),
            application
                .approval_max_input_bytes
                .unwrap_or(global.max_input_bytes),
            application
                .approval_max_concurrent
                .unwrap_or(global.max_concurrent),
            application
                .approval_max_output_tokens
                .unwrap_or(global.max_output_tokens),
            true,
        )
    }

    fn global_api_key(&self, settings: &ApprovalSettingsRecord) -> anyhow::Result<Option<String>> {
        if let Some(secret) = encrypted_secret_from_parts(
            settings.api_key_ciphertext.clone(),
            settings.api_key_nonce.clone(),
            settings.api_key_key_version,
        )? {
            return self.decrypt_api_key(&secret).map(Some);
        }
        Ok(self
            .env_api_key
            .clone()
            .filter(|value| !value.trim().is_empty()))
    }
}

fn map_test_config_error(error: anyhow::Error) -> ApprovalTestError {
    let message = error.to_string();
    if message.contains("master key")
        || message.contains("secret")
        || message.contains("decrypt")
        || message.contains("incomplete")
    {
        ApprovalTestError::SecretStorage
    } else if message.contains("API key") {
        ApprovalTestError::ApiKeyMissing
    } else {
        ApprovalTestError::InvalidConfiguration
    }
}

fn build_config(
    endpoint: String,
    model: String,
    api_key: Option<String>,
    timeout_ms: i64,
    max_input_bytes: i64,
    max_concurrent: i64,
    max_output_tokens: i64,
    application_override: bool,
) -> anyhow::Result<Option<EffectiveConfig>> {
    let endpoint = endpoint.trim().trim_end_matches('/').to_string();
    let model = model.trim().to_string();
    if endpoint.is_empty() || model.is_empty() {
        if application_override {
            bail!("application approval reviewer is not configured");
        }
        return Ok(None);
    }
    validate_endpoint(&endpoint)?;
    Ok(Some(EffectiveConfig {
        endpoint,
        model,
        api_key: api_key.unwrap_or_default(),
        timeout_ms: u64::try_from(timeout_ms)
            .unwrap_or(10_000)
            .clamp(100, 30_000),
        max_input_bytes: usize::try_from(max_input_bytes)
            .unwrap_or(128 * 1024)
            .clamp(1, 512 * 1024),
        max_concurrent: usize::try_from(max_concurrent).unwrap_or(8).clamp(1, 64),
        max_output_tokens: u32::try_from(max_output_tokens)
            .unwrap_or(1024)
            .clamp(256, 8_192),
    }))
}

fn encrypted_secret_from_parts(
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
    key_version: Option<i16>,
) -> anyhow::Result<Option<EncryptedSecret>> {
    match (ciphertext, nonce, key_version) {
        (Some(ciphertext), Some(nonce), Some(key_version)) => Ok(Some(EncryptedSecret {
            ciphertext,
            nonce,
            key_version,
        })),
        (None, None, None) => Ok(None),
        _ => bail!("stored approval reviewer API key is incomplete"),
    }
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn validate_endpoint(endpoint: &str) -> anyhow::Result<()> {
    let parsed =
        reqwest::Url::parse(endpoint).map_err(|_| anyhow::anyhow!("invalid reviewer endpoint"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        bail!("reviewer endpoint must be http(s) without embedded credentials");
    }
    Ok(())
}

pub(crate) fn encrypted_secret_from_database(
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
    key_version: Option<i16>,
) -> anyhow::Result<Option<EncryptedSecret>> {
    encrypted_secret_from_parts(ciphertext, nonce, key_version)
}

#[cfg(test)]
mod tests {
    use super::{build_config, encrypted_secret_from_database};

    #[test]
    fn global_without_endpoint_is_disabled() {
        assert!(
            build_config(
                "".to_string(),
                "model".to_string(),
                None,
                100,
                1,
                1,
                1024,
                false,
            )
            .expect("config")
            .is_none()
        );
    }

    #[test]
    fn application_requires_endpoint_and_model() {
        assert!(
            build_config(
                "https://reviewer.example/v1".to_string(),
                "".to_string(),
                Some("key".to_string()),
                100,
                1,
                1,
                1024,
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn reviewer_limits_are_clamped() {
        let config = build_config(
            "https://reviewer.example/v1".to_string(),
            "model".to_string(),
            Some("key".to_string()),
            1,
            999_999,
            999,
            999_999,
            false,
        )
        .expect("config")
        .expect("enabled");
        assert_eq!(config.timeout_ms, 100);
        assert_eq!(config.max_input_bytes, 512 * 1024);
        assert_eq!(config.max_concurrent, 64);
        assert_eq!(config.max_output_tokens, 8_192);
    }

    #[test]
    fn partial_database_secret_is_rejected() {
        assert!(encrypted_secret_from_database(Some(vec![1]), None, Some(1)).is_err());
    }
}
