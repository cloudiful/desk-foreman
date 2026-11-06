use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, bail};

use crate::policy::{ALL_SCOPES, ResourceLimits};

#[derive(Clone, Debug)]
pub struct GitJobSettings {
    pub wait: Duration,
    pub deadline: Duration,
    pub reserved_bytes: i64,
    pub max_active: i64,
    pub max_bytes: i64,
    pub max_per_binding: i64,
    pub max_per_manager: i64,
    pub max_bytes_per_manager: i64,
}

impl GitJobSettings {
    fn from_env() -> anyhow::Result<Option<Self>> {
        Self::parse(
            [
                "GIT_JOB_WAIT_SECS",
                "GIT_JOB_DEADLINE_SECS",
                "GIT_JOB_RESERVED_BYTES",
                "GIT_JOB_MAX_ACTIVE",
                "GIT_JOB_MAX_BYTES",
                "GIT_JOB_MAX_PER_BINDING",
                "GIT_JOB_MAX_PER_MANAGER",
                "GIT_JOB_MAX_BYTES_PER_MANAGER",
            ]
            .into_iter()
            .map(|key| env::var(key).ok()),
        )
    }

    fn parse(values: impl IntoIterator<Item = Option<String>>) -> anyhow::Result<Option<Self>> {
        let values: Vec<Option<String>> = values.into_iter().collect();
        let present = values.iter().filter(|value| value.is_some()).count();
        if present == 0 {
            return Ok(None);
        }
        if present != 8 {
            anyhow::bail!("incomplete GIT_JOB_* configuration");
        }
        let mut parsed = Vec::with_capacity(8);
        for value in values.into_iter().flatten() {
            let value: u64 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid GIT_JOB_* value"))?;
            let value = i64::try_from(value).map_err(|_| anyhow::anyhow!("GIT_JOB_* overflow"))?;
            if value <= 0 {
                anyhow::bail!("invalid GIT_JOB_* value");
            }
            parsed.push(value);
        }
        let wait_secs = parsed[0] as u64;
        let deadline_secs = parsed[1] as u64;
        let reserved_bytes = parsed[2];
        let max_bytes = parsed[4];
        let max_bytes_per_manager = parsed[7];
        if wait_secs > 300
            || deadline_secs < wait_secs
            || reserved_bytes > max_bytes
            || reserved_bytes > max_bytes_per_manager
        {
            anyhow::bail!("invalid GIT_JOB_* bounds");
        }
        if chrono::Duration::from_std(Duration::from_secs(deadline_secs)).is_err() {
            anyhow::bail!("invalid GIT_JOB_* duration");
        }
        Ok(Some(Self {
            wait: Duration::from_secs(wait_secs),
            deadline: Duration::from_secs(deadline_secs),
            reserved_bytes,
            max_active: parsed[3],
            max_bytes,
            max_per_binding: parsed[5],
            max_per_manager: parsed[6],
            max_bytes_per_manager,
        }))
    }
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
    pub git_jobs: Option<GitJobSettings>,
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
        let git_jobs = GitJobSettings::from_env()?;
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
            git_jobs,
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
    use super::{GitJobSettings, comma_separated_values};

    #[test]
    fn git_job_settings_reject_partial_and_invalid_bounds() {
        let full = [
            Some("1".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
        ];
        assert!(GitJobSettings::parse(full.clone()).is_ok());
        let mut partial = full.clone();
        partial[6] = None;
        assert!(GitJobSettings::parse(partial).is_err());
        let mut overflow = full.clone();
        overflow[0] = Some("18446744073709551615".to_owned());
        assert!(GitJobSettings::parse(overflow).is_err());
        let mut zero = full.clone();
        zero[3] = Some("0".to_owned());
        assert!(GitJobSettings::parse(zero).is_err());
        let mut deadline = full.clone();
        deadline[0] = Some("2".to_owned());
        deadline[1] = Some("1".to_owned());
        assert!(GitJobSettings::parse(deadline).is_err());
        let mut reservation = full.clone();
        reservation[2] = Some("2".to_owned());
        assert!(GitJobSettings::parse(reservation).is_err());
        assert!(
            GitJobSettings::parse(std::iter::repeat_n(None, 8))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn comma_separated_values_trim_and_drop_blanks() {
        assert_eq!(
            comma_separated_values("localhost, desk.example.com:443, ,127.0.0.1"),
            ["localhost", "desk.example.com:443", "127.0.0.1"]
        );
    }
}
