use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, bail};

use crate::policy::{ALL_SCOPES, ResourceLimits};

/// Public native Git gateway settings (issue 1180 P28). An absent key or base
/// URL leaves the gateway disabled: Shell still works, while `/git` and
/// capability minting fail closed with a fixed 503.
#[derive(Clone, Debug)]
pub struct GitGatewayConfig {
    pub public_base_url: Option<String>,
    pub max_body_bytes: usize,
    pub connect_timeout: Duration,
    pub head_timeout: Duration,
    pub stream_timeout: Duration,
}

impl Default for GitGatewayConfig {
    fn default() -> Self {
        Self {
            public_base_url: None,
            max_body_bytes: 256 * 1024 * 1024,
            connect_timeout: Duration::from_secs(5),
            head_timeout: Duration::from_secs(30),
            stream_timeout: Duration::from_secs(300),
        }
    }
}

impl GitGatewayConfig {
    pub fn enabled(&self) -> bool {
        self.public_base_url.is_some()
    }

    fn from_env() -> anyhow::Result<Self> {
        let public_base_url = env::var("DF_GIT_PUBLIC_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| normalize_git_base_url(&value))
            .transpose()?;
        let max_body_bytes = match env::var("DF_GIT_GATEWAY_MAX_BODY_BYTES") {
            Ok(value) => {
                let parsed = value
                    .parse::<usize>()
                    .context("DF_GIT_GATEWAY_MAX_BODY_BYTES must be a positive integer")?;
                if parsed == 0 || parsed > 1024 * 1024 * 1024 {
                    bail!("DF_GIT_GATEWAY_MAX_BODY_BYTES must be between 1 and 1073741824");
                }
                parsed
            }
            Err(_) => 256 * 1024 * 1024,
        };
        Ok(Self {
            public_base_url,
            max_body_bytes,
            ..Self::default()
        })
    }
}

fn normalize_git_base_url(raw: &str) -> anyhow::Result<String> {
    let url = reqwest::Url::parse(raw).context("DF_GIT_PUBLIC_BASE_URL must be an absolute URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("DF_GIT_PUBLIC_BASE_URL must use http or https");
    }
    if url.host_str().is_none() {
        bail!("DF_GIT_PUBLIC_BASE_URL must include a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("DF_GIT_PUBLIC_BASE_URL must not include user information");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("DF_GIT_PUBLIC_BASE_URL must not include a query or fragment");
    }
    let mut normalized = url.to_string();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

#[derive(Clone)]
pub struct AppConfig {
    pub bind_addr: String,
    pub mcp_allowed_hosts: Vec<String>,
    pub workspace_root: PathBuf,
    pub default_shell: String,
    pub session_idle_ttl: Duration,
    pub max_output_bytes: usize,
    pub server_scopes: Vec<String>,
    pub server_limits: ResourceLimits,
    pub workspace_retention: Duration,
    pub database_url: String,
    pub web_session_ttl: Duration,
    pub web_cookie_name: String,
    pub web_cookie_secure: bool,
    pub bootstrap_admin_login: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    pub bootstrap_admin_display_name: Option<String>,
    pub bootstrap_admin_email: Option<String>,
    pub bootstrap_admin_timezone: String,
    pub frontend_dist: PathBuf,
    pub build_started_at: SystemTime,
    pub git_gateway: GitGatewayConfig,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr = env::var("MCP_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
        let mcp_allowed_hosts = env::var("MCP_ALLOWED_HOSTS")
            .ok()
            .map(|value| comma_separated_values(&value))
            .unwrap_or_default();
        let workspace_root = workspace_root_from_env("WORKSPACE_ROOT")?;

        let default_shell = env::var("DEFAULT_SHELL").unwrap_or_else(|_| "bash".to_string());
        let session_idle_ttl = Duration::from_secs(
            env::var("SESSION_IDLE_TTL_SEC")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1800),
        );
        let max_output_bytes = env::var("MAX_OUTPUT_BYTES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(262_144);
        let server_scopes = env::var("SERVER_SCOPES")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(|scope| scope.trim().to_string())
                    .collect()
            })
            .unwrap_or_else(|| {
                ALL_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect()
            });
        let server_limits = ResourceLimits {
            max_timeout_ms: env::var("MAX_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .or(Some(600_000)),
            max_output_bytes: Some(max_output_bytes),
            max_file_bytes: env::var("MAX_FILE_BYTES")
                .ok()
                .and_then(|value| value.parse().ok())
                .or(Some(50 * 1024 * 1024)),
            max_sessions: env::var("MAX_SESSIONS")
                .ok()
                .and_then(|value| value.parse().ok()),
            network_enabled: env::var("NETWORK_ENABLED")
                .ok()
                .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
                .unwrap_or(true),
        };
        let workspace_retention = Duration::from_secs(
            env::var("WORKSPACE_RETENTION_DAYS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(30)
                .saturating_mul(86_400),
        );
        let database_url =
            env::var("DATABASE_URL").context("DATABASE_URL is required for web/admin features")?;
        let web_session_ttl = Duration::from_secs(
            env::var("WEB_SESSION_TTL_SEC")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(86_400 * 7),
        );
        let web_cookie_name =
            env::var("WEB_COOKIE_NAME").unwrap_or_else(|_| "desk_foreman_session".to_string());
        let web_cookie_secure = env::var("WEB_COOKIE_SECURE")
            .ok()
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false);
        let bootstrap_admin_timezone =
            env::var("BOOTSTRAP_ADMIN_TIMEZONE").unwrap_or_else(|_| "UTC".to_string());
        let frontend_dist =
            env::var("FRONTEND_DIST").unwrap_or_else(|_| "frontend/dist".to_string());

        let frontend_dist = workspace_root.join(frontend_dist);
        let git_gateway = GitGatewayConfig::from_env()?;

        Ok(Self {
            bind_addr,
            mcp_allowed_hosts,
            workspace_root,
            default_shell,
            session_idle_ttl,
            max_output_bytes,
            server_scopes,
            server_limits,
            workspace_retention,
            database_url,
            web_session_ttl,
            web_cookie_name,
            web_cookie_secure,
            bootstrap_admin_login: Some(
                env::var("BOOTSTRAP_ADMIN_LOGIN").unwrap_or_else(|_| "admin".to_string()),
            ),
            bootstrap_admin_password: Some(
                env::var("BOOTSTRAP_ADMIN_PASSWORD").unwrap_or_else(|_| "admin".to_string()),
            ),
            bootstrap_admin_display_name: env::var("BOOTSTRAP_ADMIN_DISPLAY_NAME").ok(),
            bootstrap_admin_email: env::var("BOOTSTRAP_ADMIN_EMAIL").ok(),
            bootstrap_admin_timezone,
            frontend_dist,
            build_started_at: SystemTime::now(),
            git_gateway,
        })
    }
}

fn comma_separated_values(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn workspace_root_from_env(name: &str) -> anyhow::Result<PathBuf> {
    let raw = env::var(name).unwrap_or_else(|_| "/workspace".into());
    canonical_workspace_root(name, Path::new(&raw))
}

fn canonical_workspace_root(name: &str, path: &Path) -> anyhow::Result<PathBuf> {
    let workspace_root = path
        .canonicalize()
        .with_context(|| format!("{name} must exist and be accessible"))?;
    if !workspace_root.is_dir() {
        bail!("{name} must be a directory");
    }
    Ok(workspace_root)
}

#[cfg(test)]
mod tests {
    use super::comma_separated_values;

    #[test]
    fn comma_separated_values_trim_and_drop_blanks() {
        assert_eq!(
            comma_separated_values("localhost, desk.example.com:443, ,127.0.0.1"),
            ["localhost", "desk.example.com:443", "127.0.0.1"]
        );
    }
}
