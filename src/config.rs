use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, bail};

use crate::policy::{ALL_SCOPES, ResourceLimits};

#[derive(Clone, Debug)]
pub struct RunnerClientConfig {
    pub base_url: String,
    pub auth_token: String,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub bind_addr: String,
    pub workspace_root: PathBuf,
    pub default_shell: String,
    pub session_idle_ttl: Duration,
    pub max_output_bytes: usize,
    pub server_scopes: Vec<String>,
    pub server_limits: ResourceLimits,
    pub workspace_retention: Duration,
    pub runner_client: RunnerClientConfig,
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
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr = env::var("MCP_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
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
        let runner_client = RunnerClientConfig {
            base_url: env::var("RUNNER_MANAGER_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:3001".to_string()),
            auth_token: env::var("RUNNER_MANAGER_TOKEN")
                .context("RUNNER_MANAGER_TOKEN is required for runner client")?,
        };
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

        Ok(Self {
            bind_addr,
            workspace_root,
            default_shell,
            session_idle_ttl,
            max_output_bytes,
            server_scopes,
            server_limits,
            workspace_retention,
            runner_client,
            database_url,
            web_session_ttl,
            web_cookie_name,
            web_cookie_secure,
            bootstrap_admin_login: env::var("BOOTSTRAP_ADMIN_LOGIN").ok(),
            bootstrap_admin_password: env::var("BOOTSTRAP_ADMIN_PASSWORD").ok(),
            bootstrap_admin_display_name: env::var("BOOTSTRAP_ADMIN_DISPLAY_NAME").ok(),
            bootstrap_admin_email: env::var("BOOTSTRAP_ADMIN_EMAIL").ok(),
            bootstrap_admin_timezone,
            frontend_dist,
            build_started_at: SystemTime::now(),
        })
    }
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
