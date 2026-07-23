use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::bail;
use desk_foreman_approval::{ApprovalMode, ApprovalReviewer, OpenAiReviewer, OpenAiReviewerConfig};
use tokio::sync::RwLock;

use crate::{AppState, actor::ActorContext, db::types::ApprovalSettingsRecord};

type Reviewer = Arc<dyn ApprovalReviewer>;

#[derive(Clone)]
pub struct ApprovalService {
    api_key: Option<String>,
    cache: Arc<RwLock<HashMap<String, Reviewer>>>,
    database_backed: bool,
}

impl ApprovalService {
    pub fn from_env() -> Self {
        Self {
            api_key: std::env::var("APPROVAL_API_KEY")
                .ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            cache: Arc::new(RwLock::new(HashMap::new())),
            database_backed: true,
        }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            api_key: None,
            cache: Arc::new(RwLock::new(HashMap::new())),
            database_backed: false,
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
        let global = crate::db::queries::get_approval_settings(&state.db).await?;
        let Some(config) = effective_config(actor, &global)? else {
            return Ok(None);
        };
        if config.endpoint.is_empty() || config.model.is_empty() {
            return Ok(None);
        }
        if self.api_key.as_deref().is_none_or(str::is_empty) {
            anyhow::bail!("approval reviewer API key is not configured");
        }

        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            config.endpoint,
            config.model,
            config.timeout_ms,
            config.max_input_bytes,
            config.max_concurrent
        );
        if let Some(reviewer) = self.cache.read().await.get(&key).cloned() {
            return Ok(Some(reviewer));
        }

        let reviewer = Arc::new(OpenAiReviewer::new(OpenAiReviewerConfig {
            api_base: config.endpoint,
            api_key: self.api_key.clone(),
            model: config.model,
            timeout: Duration::from_millis(config.timeout_ms),
            max_input_bytes: config.max_input_bytes,
            max_concurrent: config.max_concurrent,
        })?) as Reviewer;
        self.cache.write().await.insert(key, reviewer.clone());
        Ok(Some(reviewer))
    }
}

#[derive(Clone, Debug)]
struct EffectiveConfig {
    endpoint: String,
    model: String,
    timeout_ms: u64,
    max_input_bytes: usize,
    max_concurrent: usize,
}

fn effective_config(
    actor: &ActorContext,
    global: &ApprovalSettingsRecord,
) -> anyhow::Result<Option<EffectiveConfig>> {
    let (endpoint, model, application_override) = if let Some(application) = &actor.application {
        let mode = ApprovalMode::parse(&application.approval_mode)
            .ok_or_else(|| anyhow::anyhow!("invalid application approval mode"))?;
        match mode {
            ApprovalMode::Disabled => return Ok(None),
            ApprovalMode::Enabled => (
                application.approval_endpoint.clone().unwrap_or_default(),
                application.approval_model.clone().unwrap_or_default(),
                true,
            ),
            ApprovalMode::Inherit => (
                global.endpoint.clone().unwrap_or_default(),
                global.model.clone().unwrap_or_default(),
                false,
            ),
        }
    } else {
        (
            global.endpoint.clone().unwrap_or_default(),
            global.model.clone().unwrap_or_default(),
            false,
        )
    };
    let endpoint = endpoint.trim().trim_end_matches('/').to_string();
    let model = model.trim().to_string();
    if endpoint.is_empty() || model.is_empty() {
        if application_override {
            anyhow::bail!("application approval reviewer is not configured");
        }
        return Ok(None);
    }
    validate_endpoint(&endpoint)?;
    let timeout_ms = u64::try_from(global.timeout_ms)
        .unwrap_or(10_000)
        .clamp(100, 30_000);
    let max_input_bytes = usize::try_from(global.max_input_bytes)
        .unwrap_or(128 * 1024)
        .clamp(1, 512 * 1024);
    let max_concurrent = usize::try_from(global.max_concurrent)
        .unwrap_or(8)
        .clamp(1, 64);
    Ok(Some(EffectiveConfig {
        endpoint,
        model,
        timeout_ms,
        max_input_bytes,
        max_concurrent,
    }))
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
