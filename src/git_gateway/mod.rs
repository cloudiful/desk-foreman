//! Native Git Smart HTTP gateway (issue 1180 P28).
//!
//! Public routes proxy only four fixed Smart HTTP shapes to the admin-configured
//! upstream. Upstream credentials are read inside the request path; callers
//! authenticate with a short-lived workspace-scoped capability instead.

pub(crate) mod capability;
mod limits;
pub(crate) mod proxy;
pub mod signing_key;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Duration, Utc};

use crate::{config::AppConfig, db::types::WorkspaceBindingResponse};

pub(crate) fn gateway_enabled(config: &AppConfig) -> bool {
    config.git_gateway.enabled()
}

pub(crate) fn public_base_url(config: &AppConfig) -> Option<&str> {
    config.git_gateway.public_base_url.as_deref()
}

pub(crate) fn remote_url(
    config: &AppConfig,
    workspace_binding_id: i64,
    remote_alias: &str,
) -> Option<String> {
    let base = public_base_url(config)?;
    Some(format!("{base}/git/{workspace_binding_id}/{remote_alias}"))
}

pub(crate) fn capability_expiry(
    binding: &WorkspaceBindingResponse,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let mut expiry = now + Duration::seconds(capability::CAPABILITY_TTL_SECONDS);
    if binding.resource_kind.is_some()
        && let Some(lease_expiry) = binding.write_lease_expires_at
        && lease_expiry < expiry
    {
        expiry = lease_expiry;
    }
    expiry
}
