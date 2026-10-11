use std::path::{Path, PathBuf};

use anyhow::bail;

pub(crate) fn native_task_root_from_env() -> anyhow::Result<Option<PathBuf>> {
    let Some(raw) = std::env::var("RUNNER_NATIVE_TASK_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    parse_native_task_root(&raw).map(Some)
}

pub(crate) fn native_task_host_root_from_env() -> anyhow::Result<Option<PathBuf>> {
    let Some(raw) = std::env::var("RUNNER_NATIVE_TASK_HOST_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    parse_native_task_host_root(&raw).map(Some)
}

fn parse_native_task_root(raw: &str) -> anyhow::Result<PathBuf> {
    validate_absolute_path(raw, false)?;
    let path = PathBuf::from(raw);
    let metadata = std::fs::metadata(&path)
        .map_err(|_| anyhow::anyhow!("RUNNER_NATIVE_TASK_ROOT must exist and be accessible"))?;
    if !metadata.is_dir() {
        bail!("RUNNER_NATIVE_TASK_ROOT must be a directory");
    }
    Ok(path)
}

pub(crate) fn parse_native_task_host_root(raw: &str) -> anyhow::Result<PathBuf> {
    validate_absolute_path(raw, true)?;
    Ok(PathBuf::from(raw))
}

pub(crate) fn validate_native_task_host_root(path: &Path) -> anyhow::Result<()> {
    let raw = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("RUNNER_NATIVE_TASK_HOST_ROOT must be valid UTF-8"))?;
    validate_absolute_path(raw, true)
}

fn validate_absolute_path(raw: &str, require_normalized: bool) -> anyhow::Result<()> {
    if !raw.starts_with('/') {
        bail!("native task root must be an absolute path");
    }
    if require_normalized
        && (raw.contains(',')
            || raw.contains('\\')
            || raw.chars().any(char::is_control)
            || (raw != "/" && raw.ends_with('/')))
    {
        bail!("RUNNER_NATIVE_TASK_HOST_ROOT contains an unsupported path character");
    }
    let components = raw[1..].split('/');
    for component in components {
        if component == "." || component == ".." {
            bail!("native task roots must not contain relative path components");
        }
        if require_normalized && raw != "/" && component.is_empty() {
            bail!("RUNNER_NATIVE_TASK_HOST_ROOT must be normalized");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_native_task_host_root, parse_native_task_root, validate_native_task_host_root,
    };

    #[test]
    fn host_root_requires_a_normalized_absolute_daemon_path() {
        assert_eq!(
            parse_native_task_host_root("/daemon/native-tasks").expect("valid root"),
            std::path::PathBuf::from("/daemon/native-tasks")
        );
        for path in [
            "relative/root",
            "/daemon/./tasks",
            "/daemon/../tasks",
            "/daemon//tasks",
            "/daemon/tasks/",
            "/daemon/tasks,other",
            "/daemon\\tasks",
            "/daemon/\ntasks",
        ] {
            assert!(parse_native_task_host_root(path).is_err(), "{path:?}");
        }
        assert!(validate_native_task_host_root(std::path::Path::new("relative")).is_err());
    }

    #[test]
    fn local_root_retains_the_configured_symlink_for_use_time_rejection() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        std::fs::create_dir(&real).expect("real root");
        let alias = temp.path().join("alias");
        symlink(&real, &alias).expect("root alias");

        assert_eq!(
            parse_native_task_root(alias.to_str().expect("utf-8 path")).expect("configured root"),
            alias
        );
        assert!(parse_native_task_root("relative/root").is_err());
    }
}
