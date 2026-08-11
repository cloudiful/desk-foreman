use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

use crate::{
    db::{
        queries::external_user_hash,
        types::{ApplicationResponse, UserRecord, WorkspaceBindingResponse},
    },
    error::AppError,
};

pub fn default_user_workspace(base_root: &Path, user_id: i64) -> PathBuf {
    base_root.join("users").join(user_id.to_string())
}

pub fn default_application_workspace(
    base_root: &Path,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
) -> PathBuf {
    base_root
        .join("apps")
        .join(application_id.to_string())
        .join("users")
        .join(external_user_hash(external_user_id))
        .join(workspace_key)
}

pub fn resolve_user_workspace(base_root: &Path, user: &UserRecord) -> anyhow::Result<PathBuf> {
    let configured = user
        .workspace_root
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let candidate = match configured {
        Some(path) => PathBuf::from(path),
        None => default_user_workspace(base_root, user.user_id),
    };
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        base_root.join(candidate)
    };

    resolve_absolute_workspace(base_root, absolute)
}

pub fn resolve_workspace_binding_root(
    base_root: &Path,
    binding: &WorkspaceBindingResponse,
) -> anyhow::Result<PathBuf> {
    resolve_absolute_workspace(base_root, PathBuf::from(&binding.workspace_root))
}

pub fn resolve_application_workspace(
    base_root: &Path,
    application: &ApplicationResponse,
    external_user_id: &str,
    workspace_key: &str,
) -> anyhow::Result<PathBuf> {
    resolve_absolute_workspace(
        base_root,
        default_application_workspace(
            base_root,
            application.application_id,
            external_user_id,
            workspace_key,
        ),
    )
}

/// Resolves a shared resource-owned workspace for an application.
///
/// Resource workspaces (e.g. `code_project:<id>`) are shared across external
/// users, so the path is derived from the resource identity rather than the
/// requesting user.
pub fn resolve_resource_workspace(
    base_root: &Path,
    application: &ApplicationResponse,
    resource_kind: &str,
    resource_id: &str,
) -> anyhow::Result<PathBuf> {
    let resource_hash = resource_workspace_hash(resource_kind, resource_id);
    resolve_absolute_workspace(
        base_root,
        base_root
            .join("apps")
            .join(application.application_id.to_string())
            .join("resources")
            .join(&resource_hash),
    )
}

/// Parses a resource workspace key of the form `kind:id`.
///
/// Returns `None` for keys that do not look like a resource workspace or that
/// contain characters unsafe for workspace paths.
pub fn parse_resource_workspace_key(key: &str) -> Option<(String, String)> {
    let (kind, id) = key.split_once(':')?;
    if kind.is_empty()
        || id.is_empty()
        || id.len() > 128
        || !kind.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return None;
    }
    Some((kind.to_string(), id.to_string()))
}

fn resource_workspace_hash(resource_kind: &str, resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(resource_kind.as_bytes());
    hasher.update(b":");
    hasher.update(resource_id.as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn initialize_workspace_template(
    base_root: &Path,
    workspace_root: &Path,
    template: Option<&str>,
) -> Result<(), AppError> {
    let Some(template_name) = template.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    if workspace_root
        .read_dir()
        .map_err(|error| AppError::internal(error.into()))?
        .next()
        .is_some()
    {
        return Ok(());
    }
    let template_root = base_root.join("templates").join(template_name);
    if !template_root.is_dir() {
        return Ok(());
    }
    copy_dir_contents(&template_root, workspace_root).map_err(AppError::internal)
}

fn resolve_absolute_workspace(base_root: &Path, absolute: PathBuf) -> anyhow::Result<PathBuf> {
    if !absolute.starts_with(base_root) {
        bail!("workspace path escapes base workspace root");
    }

    fs::create_dir_all(&absolute)
        .with_context(|| format!("failed to create workspace {}", absolute.display()))?;
    let canonical = absolute
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace {}", absolute.display()))?;
    let canonical_base = base_root
        .canonicalize()
        .with_context(|| format!("failed to resolve base workspace {}", base_root.display()))?;
    if !canonical.starts_with(&canonical_base) {
        bail!("workspace path escapes base workspace root");
    }
    Ok(canonical)
}

fn copy_dir_contents(from: &Path, to: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("failed to read {}", from.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&target_path)?;
            copy_dir_contents(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "failed to copy template file {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{default_application_workspace, default_user_workspace, resolve_user_workspace};
    use crate::db::types::UserRecord;
    use chrono::Utc;
    use std::path::Path;

    fn user(workspace_root: Option<String>) -> UserRecord {
        UserRecord {
            user_id: 42,
            login_name: "alice".to_string(),
            password_hash: "hash".to_string(),
            display_name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            timezone: "UTC".to_string(),
            workspace_root,
            is_admin: false,
            is_active: true,
            deleted_at: None,
            last_login_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn uses_default_workspace_and_creates_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_user_workspace(temp.path(), &user(None)).expect("workspace");
        let expected = default_user_workspace(temp.path(), 42)
            .canonicalize()
            .expect("canonical default workspace");
        assert_eq!(resolved, expected);
        assert!(resolved.is_dir());
    }

    #[test]
    fn rejects_relative_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let error = resolve_user_workspace(temp.path(), &user(Some("../escape".to_string())))
            .expect_err("should reject escape");
        assert!(error.to_string().contains("escapes base workspace root"));
    }

    #[test]
    fn rejects_absolute_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let error = resolve_user_workspace(temp.path(), &user(Some("/tmp/escape".to_string())))
            .expect_err("should reject absolute escape");
        assert!(error.to_string().contains("escapes base workspace root"));
    }

    #[test]
    fn application_workspace_uses_hashed_external_user() {
        let workspace = default_application_workspace(
            Path::new("/workspace"),
            7,
            "alice@example.com",
            "default",
        );
        let rendered = workspace.to_string_lossy();
        assert!(rendered.starts_with("/workspace/apps/7/users/"));
        assert!(!rendered.contains("alice@example.com"));
        assert!(rendered.ends_with("/default"));
    }
}

#[cfg(test)]
mod resource_tests {
    use super::parse_resource_workspace_key;

    #[test]
    fn parses_valid_resource_keys() {
        assert_eq!(
            parse_resource_workspace_key("code_project:123e4567-e89b-12d3-a456-426614174000"),
            Some((
                "code_project".to_string(),
                "123e4567-e89b-12d3-a456-426614174000".to_string()
            ))
        );
        assert_eq!(
            parse_resource_workspace_key("code_project:project_1"),
            Some(("code_project".to_string(), "project_1".to_string()))
        );
    }

    #[test]
    fn rejects_malformed_resource_keys() {
        assert_eq!(parse_resource_workspace_key("default"), None);
        assert_eq!(parse_resource_workspace_key("code_project:"), None);
        assert_eq!(parse_resource_workspace_key(":id"), None);
        assert_eq!(parse_resource_workspace_key("code_project:a/b"), None);
        assert_eq!(parse_resource_workspace_key("code project:abc"), None);
        assert_eq!(parse_resource_workspace_key("CODE:abc"), None);
        assert_eq!(
            parse_resource_workspace_key(&format!("code_project:{}", "x".repeat(200))),
            None
        );
    }
}
