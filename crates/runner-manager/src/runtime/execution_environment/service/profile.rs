//! Optional trusted service profile for execution environments
//! (issue 749 P2a).
//!
//! The service profile describes how a private agent service is reached:
//! the Docker network it joins and the trusted environment-variable name
//! carrying its per-environment credential. It deliberately holds no
//! credential value — only the *name* the credential is stored under — so
//! the token itself never becomes configuration.
//!
//! All four settings are all-or-none. Setting any subset is a
//! misconfiguration rather than a partially enabled feature, because a
//! half-configured service would either dial an untrusted address or leave
//! the agent without the credential it expects.

/// Upper bound on the Docker network name.
const MAX_NETWORK_LEN: usize = 63;
/// Upper bound on the basic-auth username.
const MAX_USERNAME_LEN: usize = 64;
/// Upper bound on the trusted environment-variable name.
const MAX_ENV_NAME_LEN: usize = 64;

/// A validated, fully specified service profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceProfileConfig {
    /// Docker network the environment container must join.
    pub network: String,
    /// Port the private agent service listens on inside that network.
    pub port: u16,
    /// Username presented to the agent's basic-auth challenge.
    pub auth_username: String,
    /// Environment-variable name the agent reads its credential from.
    pub auth_env_name: String,
}

/// The four raw settings as read from the environment, before validation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServiceProfileEnv<'a> {
    pub network: Option<&'a str>,
    pub port: Option<&'a str>,
    pub auth_username: Option<&'a str>,
    pub auth_env_name: Option<&'a str>,
}

impl ServiceProfileEnv<'_> {
    fn present(&self) -> usize {
        [
            self.network,
            self.port,
            self.auth_username,
            self.auth_env_name,
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .count()
    }
}

/// Trusted service configuration, degraded rather than defaulted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ServiceProfile {
    /// Not configured. The environment runs exactly as it did before
    /// service support existed.
    #[default]
    Unset,
    /// Explicitly misconfigured. Carries a redacted reason naming the
    /// setting, never the configured value.
    Invalid(String),
    /// Validated and usable.
    Configured(ServiceProfileConfig),
}

fn invalid(reason: &'static str) -> ServiceProfile {
    ServiceProfile::Invalid(
        String::from("execution environment service profile is invalid: ") + reason,
    )
}

/// Parse and validate the four service settings together.
pub(crate) fn parse_service_profile(raw: ServiceProfileEnv<'_>) -> ServiceProfile {
    match raw.present() {
        0 => return ServiceProfile::Unset,
        4 => {}
        _ => {
            return invalid(
                "all four RUNNER_EXECUTION_ENVIRONMENT_SERVICE_* settings must be configured \
                 together; enabling only some of them is refused",
            );
        }
    }
    let network = raw.network.unwrap_or_default().trim();
    let auth_username = raw.auth_username.unwrap_or_default().trim();
    let auth_env_name = raw.auth_env_name.unwrap_or_default().trim();

    if let Err(reason) = validate_network(network) {
        return invalid(reason);
    }
    let port = match parse_port(raw.port.unwrap_or_default()) {
        Ok(port) => port,
        Err(reason) => return invalid(reason),
    };
    if let Err(reason) = validate_username(auth_username) {
        return invalid(reason);
    }
    if let Err(reason) = validate_env_name(auth_env_name) {
        return invalid(reason);
    }
    ServiceProfile::Configured(ServiceProfileConfig {
        network: network.to_string(),
        port,
        auth_username: auth_username.to_string(),
        auth_env_name: auth_env_name.to_string(),
    })
}

/// Docker network names are restricted to an ASCII alphanumeric head and a
/// bounded alphanumeric/underscore/dash/dot tail.
fn validate_network(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_NETWORK_LEN {
        return Err("the network name must be 1-63 characters");
    }
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err("the network name must not be empty");
    };
    if !first.is_ascii_alphanumeric() {
        return Err("the network name must start with an ASCII letter or digit");
    }
    if !characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err("the network name must be ASCII alphanumeric with _ - . only");
    }
    Ok(())
}

fn parse_port(raw: &str) -> Result<u16, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err("the service port must be a decimal number");
    }
    match raw.parse::<u32>() {
        Ok(port) if (1..=65_535).contains(&port) => Ok(port as u16),
        _ => Err("the service port must be between 1 and 65535"),
    }
}

/// A basic-auth username may not contain a colon, because that would make
/// the credential ambiguous once it is joined into `user:password`.
fn validate_username(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_USERNAME_LEN {
        return Err("the service auth username must be 1-64 characters");
    }
    if !value.chars().all(|c| c.is_ascii_graphic()) {
        return Err("the service auth username must be printable ASCII");
    }
    if value.contains(':') {
        return Err("the service auth username must not contain a colon");
    }
    Ok(())
}

/// POSIX environment-variable name.
fn validate_env_name(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_ENV_NAME_LEN {
        return Err("the service auth env name must be 1-64 characters");
    }
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err("the service auth env name must not be empty");
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err("the service auth env name must start with an ASCII letter or underscore");
    }
    if !characters.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("the service auth env name must be ASCII alphanumeric with _ only");
    }
    Ok(())
}
