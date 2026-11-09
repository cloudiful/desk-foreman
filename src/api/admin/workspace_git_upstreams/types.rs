//! Request schemas for workspace Git upstream administration (issue 1180 P27).
//!
//! Request types deliberately derive neither `Debug` nor `Serialize`: they can
//! carry a plaintext secret, so they must not be logged or echoed accidentally.

use std::borrow::Cow;

use serde::Deserialize;
use utoipa::ToSchema;
use validator::{Validate, ValidationError, ValidationErrors};

const MAX_ALIAS_LEN: usize = 64;
const MAX_URL_LEN: usize = 2048;
const MAX_USERNAME_LEN: usize = 256;
const MAX_SECRET_LEN: usize = 8192;

#[derive(Deserialize, ToSchema)]
pub struct CreateWorkspaceGitUpstreamRequest {
    pub remote_alias: String,
    pub target_url: String,
    #[serde(default)]
    pub credential_username: Option<String>,
    #[serde(default)]
    pub credential_secret: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateWorkspaceGitUpstreamRequest {
    #[serde(default)]
    pub remote_alias: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub credential_username: Option<String>,
    #[serde(default)]
    pub credential_secret: Option<String>,
    #[serde(default)]
    pub clear_credential: bool,
}

impl Validate for CreateWorkspaceGitUpstreamRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        check_alias(&self.remote_alias, &mut errors, "remote_alias");
        check_url(&self.target_url, &mut errors, "target_url");
        check_credential_pair(
            self.credential_username.as_deref(),
            self.credential_secret.as_deref(),
            false,
            &mut errors,
        );
        finish(errors)
    }
}

impl Validate for UpdateWorkspaceGitUpstreamRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        if let Some(alias) = self.remote_alias.as_deref() {
            check_alias(alias, &mut errors, "remote_alias");
        }
        if let Some(url) = self.target_url.as_deref() {
            check_url(url, &mut errors, "target_url");
        }
        check_credential_pair(
            self.credential_username.as_deref(),
            self.credential_secret.as_deref(),
            self.clear_credential,
            &mut errors,
        );
        finish(errors)
    }
}

fn check_credential_pair(
    username: Option<&str>,
    secret: Option<&str>,
    clear_credential: bool,
    errors: &mut ValidationErrors,
) {
    if clear_credential {
        if username.is_some() || secret.is_some() {
            errors.add(
                "clear_credential",
                invalid(
                    "clear_credential cannot be combined with credential_username or credential_secret",
                ),
            );
        }
        return;
    }
    match (username, secret) {
        (None, None) => {}
        (Some(username), Some(secret)) => {
            check_username(username, errors);
            check_secret(secret, errors);
        }
        _ => errors.add(
            "credential_secret",
            invalid("credential_username and credential_secret must be provided together"),
        ),
    }
}

fn check_alias(value: &str, errors: &mut ValidationErrors, field: &'static str) {
    if !is_valid_alias(value) {
        errors.add(
            field,
            invalid(
                "must start with an ASCII letter or digit and contain only ASCII letters, digits, '_' or '-' (max 64)",
            ),
        );
    }
}

fn check_url(value: &str, errors: &mut ValidationErrors, field: &'static str) {
    if let Err(message) = validate_target_url(value) {
        errors.add(field, invalid(message));
    }
}

fn check_username(value: &str, errors: &mut ValidationErrors) {
    if value.len() > MAX_USERNAME_LEN {
        errors.add("credential_username", invalid("must be at most 256 bytes"));
    } else if contains_line_break_or_nul(value) {
        errors.add(
            "credential_username",
            invalid("must not contain NUL, CR or LF characters"),
        );
    }
}

fn check_secret(value: &str, errors: &mut ValidationErrors) {
    if value.is_empty() {
        errors.add("credential_secret", invalid("must not be empty"));
    } else if value.len() > MAX_SECRET_LEN {
        errors.add("credential_secret", invalid("must be at most 8192 bytes"));
    } else if contains_line_break_or_nul(value) {
        errors.add(
            "credential_secret",
            invalid("must not contain NUL, CR or LF characters"),
        );
    }
}

pub fn is_valid_alias(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_ALIAS_LEN {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if index == 0 {
            byte.is_ascii_alphanumeric()
        } else {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        }
    })
}

pub fn validate_target_url(value: &str) -> Result<(), &'static str> {
    if value.len() > MAX_URL_LEN {
        return Err("must be at most 2048 bytes");
    }
    if contains_line_break_or_nul(value) {
        return Err("must not contain NUL, CR or LF characters");
    }
    if value.chars().any(|character| character.is_control()) {
        return Err("must not contain control characters");
    }
    let url = reqwest::Url::parse(value).map_err(|_| "must be a valid absolute URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("must use http or https");
    }
    if url.host_str().is_none() {
        return Err("must include a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("must not include user information");
    }
    if url.query().is_some() {
        return Err("must not include a query string");
    }
    if url.fragment().is_some() {
        return Err("must not include a fragment");
    }
    Ok(())
}

fn contains_line_break_or_nul(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
}

fn invalid(message: impl Into<String>) -> ValidationError {
    let mut error = ValidationError::new("validation");
    error.message = Some(Cow::Owned(message.into()));
    error
}

fn finish(errors: ValidationErrors) -> Result<(), ValidationErrors> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_strict_ascii_identifiers() {
        assert!(is_valid_alias("origin"));
        assert!(is_valid_alias("up_stream-2"));
        assert!(!is_valid_alias(""));
        assert!(!is_valid_alias("-lead"));
        assert!(!is_valid_alias("has space"));
        assert!(!is_valid_alias("ümlaut"));
        assert!(!is_valid_alias(&"a".repeat(MAX_ALIAS_LEN + 1)));
    }

    #[test]
    fn urls_are_bounded_http_smart_endpoints() {
        assert!(validate_target_url("https://git.example.com/team/repo.git").is_ok());
        assert!(validate_target_url("http://git.example.com/repo.git").is_ok());
        assert!(validate_target_url("ssh://git.example.com/repo.git").is_err());
        assert!(validate_target_url("file:///tmp/repo.git").is_err());
        assert!(validate_target_url("https://user:pass@git.example.com/repo.git").is_err());
        assert!(validate_target_url("https://git.example.com/repo.git?x=1").is_err());
        assert!(validate_target_url("https://git.example.com/repo.git#main").is_err());
        assert!(validate_target_url("https://").is_err());
        assert!(validate_target_url("https://git.example.com/repo\n.git").is_err());
    }

    #[test]
    fn create_requires_a_complete_credential_pair() {
        let missing_secret = CreateWorkspaceGitUpstreamRequest {
            remote_alias: "origin".into(),
            target_url: "https://git.example.com/repo.git".into(),
            credential_username: Some("user".into()),
            credential_secret: None,
        };
        assert!(missing_secret.validate().is_err());

        let complete = CreateWorkspaceGitUpstreamRequest {
            remote_alias: "origin".into(),
            target_url: "https://git.example.com/repo.git".into(),
            credential_username: Some("user".into()),
            credential_secret: Some("secret".into()),
        };
        assert!(complete.validate().is_ok());

        let empty_secret = CreateWorkspaceGitUpstreamRequest {
            remote_alias: "origin".into(),
            target_url: "https://git.example.com/repo.git".into(),
            credential_username: Some("user".into()),
            credential_secret: Some(String::new()),
        };
        assert!(empty_secret.validate().is_err());
    }

    #[test]
    fn update_clear_is_exclusive_with_replacement() {
        let clear = UpdateWorkspaceGitUpstreamRequest {
            remote_alias: None,
            target_url: None,
            credential_username: None,
            credential_secret: None,
            clear_credential: true,
        };
        assert!(clear.validate().is_ok());

        let conflicting = UpdateWorkspaceGitUpstreamRequest {
            remote_alias: None,
            target_url: None,
            credential_username: Some("user".into()),
            credential_secret: Some("secret".into()),
            clear_credential: true,
        };
        assert!(conflicting.validate().is_err());

        let partial = UpdateWorkspaceGitUpstreamRequest {
            remote_alias: None,
            target_url: None,
            credential_username: Some("user".into()),
            credential_secret: None,
            clear_credential: false,
        };
        assert!(partial.validate().is_err());
    }
}
