//! Pure planning for the isolated trusted-Git helper.
//!
//! Nothing here starts Docker, resolves an image, or mutates a repository.
//! The plan is built only from manager-owned private paths and the closed
//! verification command enum.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::verify::TrustedGitCommand;

pub(crate) const CONTAINER_OPERATION_ROOT: &str = "/git-verify";
pub(crate) const CONTAINER_REPO_DIR: &str = "/git-verify/repo";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MountMode {
    ReadOnly,
    Writable,
}

impl MountMode {
    pub(super) fn for_command(command: &TrustedGitCommand) -> Self {
        if matches!(command, TrustedGitCommand::Init { .. }) {
            Self::Writable
        } else {
            Self::ReadOnly
        }
    }

    pub(super) fn suffix(self) -> &'static str {
        match self {
            Self::ReadOnly => "ro",
            Self::Writable => "rw",
        }
    }
}

/// Resource and image policy derived only from the existing runner-manager
/// configuration. A request can never override any of these values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitVerifyLimits {
    pub(crate) timeout: Duration,
    pub(crate) max_output_bytes: usize,
    pub(crate) memory_limit: String,
    pub(crate) cpu_limit: String,
    pub(crate) pids_limit: u64,
    pub(crate) image: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitVerifyConfigError {
    InvalidLimits,
    InvalidMemoryLimit,
    InvalidCpuLimit,
    UnpinnedImage,
    InvalidImage,
}

impl std::fmt::Display for GitVerifyConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidLimits => "git verification limits are invalid",
            Self::InvalidMemoryLimit => "git verification memory limit is invalid",
            Self::InvalidCpuLimit => "git verification cpu limit is invalid",
            Self::UnpinnedImage => "git verification image is not digest-pinned",
            Self::InvalidImage => "git verification image is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GitVerifyConfigError {}

impl GitVerifyLimits {
    pub(crate) fn from_config(
        config: &crate::config::RunnerManagerConfig,
    ) -> Result<Self, GitVerifyConfigError> {
        if config.max_timeout_ms == 0 || config.max_output_bytes == 0 || config.pids_limit == 0 {
            return Err(GitVerifyConfigError::InvalidLimits);
        }
        if !is_valid_memory_limit(&config.memory_limit) {
            return Err(GitVerifyConfigError::InvalidMemoryLimit);
        }
        if !is_valid_cpu_limit(&config.cpu_limit) {
            return Err(GitVerifyConfigError::InvalidCpuLimit);
        }
        let image = config
            .git_image
            .as_deref()
            .ok_or(GitVerifyConfigError::UnpinnedImage)?;
        validate_pinned_image(image).map_err(|error| match error {
            ImageError::Unpinned => GitVerifyConfigError::UnpinnedImage,
            ImageError::Invalid => GitVerifyConfigError::InvalidImage,
        })?;
        Ok(Self {
            timeout: Duration::from_millis(config.max_timeout_ms),
            max_output_bytes: config.max_output_bytes,
            memory_limit: config.memory_limit.clone(),
            cpu_limit: config.cpu_limit.clone(),
            pids_limit: config.pids_limit,
            image: image.to_owned(),
        })
    }

    fn validate(&self) -> Result<(), HelperPlanError> {
        if self.timeout.is_zero()
            || self.max_output_bytes == 0
            || self.pids_limit == 0
            || !is_valid_memory_limit(&self.memory_limit)
            || !is_valid_cpu_limit(&self.cpu_limit)
        {
            return Err(HelperPlanError::InvalidLimits);
        }
        validate_pinned_image(&self.image).map_err(|error| match error {
            ImageError::Unpinned => HelperPlanError::UnpinnedImage,
            ImageError::Invalid => HelperPlanError::InvalidImage,
        })?;
        Ok(())
    }
}

/// Manager-owned paths for one verification operation. `repo_dir`, `home_dir`
/// and `template_dir` are all under `operation_root`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelperPaths {
    pub(crate) operation_root: PathBuf,
    pub(crate) repo_dir: PathBuf,
    pub(crate) home_dir: PathBuf,
    pub(crate) template_dir: PathBuf,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

/// One closed Docker invocation plus its matching cleanup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelperPlan {
    container_name: String,
    docker_args: Vec<String>,
    cleanup_args: Vec<String>,
    deadline: Duration,
    max_output_bytes: usize,
    mount_mode: MountMode,
    scratch_bytes: usize,
}

impl HelperPlan {
    pub(super) fn container_name(&self) -> &str {
        &self.container_name
    }

    pub(super) fn docker_args(&self) -> &[String] {
        &self.docker_args
    }

    pub(super) fn cleanup_args(&self) -> &[String] {
        &self.cleanup_args
    }

    pub(super) fn deadline(&self) -> Duration {
        self.deadline
    }

    pub(super) fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub(super) fn validate_for_invocation(
        &self,
        command: &TrustedGitCommand,
    ) -> Result<(), HelperPlanError> {
        self.validate_for_execution()?;
        if self.mount_mode != MountMode::for_command(command) {
            return Err(HelperPlanError::InvalidCommandArgument);
        }
        Ok(())
    }

    pub(super) fn validate_for_execution(&self) -> Result<(), HelperPlanError> {
        if self.deadline.is_zero() || self.max_output_bytes == 0 || self.docker_args.len() < 3 {
            return Err(HelperPlanError::InvalidLimits);
        }
        if self.docker_args[0] != "docker" || self.docker_args[1] != "run" {
            return Err(HelperPlanError::InvalidCommandArgument);
        }
        if !self
            .docker_args
            .iter()
            .any(|argument| argument == "--pull=never")
            || !self.docker_args.iter().any(|argument| argument == "--rm")
            || !self
                .docker_args
                .iter()
                .any(|argument| argument == "--read-only")
        {
            return Err(HelperPlanError::InvalidCommandArgument);
        }
        let name =
            argument_after(&self.docker_args, "--name").ok_or(HelperPlanError::UnsafeName)?;
        if name != self.container_name || !is_safe_container_name(name) {
            return Err(HelperPlanError::UnsafeName);
        }
        if self
            .docker_args
            .iter()
            .filter(|argument| *argument == "--mount")
            .count()
            != 1
        {
            return Err(HelperPlanError::UnsafePath);
        }
        let mount =
            argument_after(&self.docker_args, "--mount").ok_or(HelperPlanError::UnsafePath)?;
        let target_suffix = format!(",target=/git-verify,{}", self.mount_mode.suffix());
        if !mount.starts_with("type=bind,source=") || !mount.ends_with(&target_suffix) {
            return Err(HelperPlanError::UnsafePath);
        }
        for forbidden in [
            "--privileged",
            "--pid",
            "--ipc",
            "--device",
            "-v",
            "--volume",
            "--env-file",
        ] {
            if self
                .docker_args
                .iter()
                .any(|argument| argument == forbidden)
            {
                return Err(HelperPlanError::UnsafePath);
            }
        }
        let scratch = self.scratch_bytes.to_string();
        let ulimit = format!("fsize={scratch}");
        if ["--tmpfs", "--shm-size", "--ulimit"].iter().any(|flag| {
            self.docker_args
                .iter()
                .filter(|argument| argument == flag)
                .count()
                != 1
        }) || argument_after(&self.docker_args, "--network") != Some("none")
            || argument_after(&self.docker_args, "--workdir") != Some(CONTAINER_OPERATION_ROOT)
            || !argument_after(&self.docker_args, "--tmpfs").is_some_and(|value| {
                value.starts_with("/tmp:") && value.ends_with(&format!("size={scratch}"))
            })
            || argument_after(&self.docker_args, "--shm-size") != Some(scratch.as_str())
            || argument_after(&self.docker_args, "--ulimit") != Some(ulimit.as_str())
            || argument_after(&self.docker_args, "--cap-drop") != Some("ALL")
            || argument_after(&self.docker_args, "--security-opt") != Some("no-new-privileges:true")
        {
            return Err(HelperPlanError::UnsafePath);
        }
        let user =
            argument_after(&self.docker_args, "--user").ok_or(HelperPlanError::UnsafePath)?;
        let Some((uid, gid)) = user.split_once(':') else {
            return Err(HelperPlanError::UnsafePath);
        };
        if uid.parse::<u32>().ok().filter(|uid| *uid > 0).is_none()
            || gid.parse::<u32>().ok().filter(|gid| *gid > 0).is_none()
        {
            return Err(HelperPlanError::RootUser);
        }
        argument_after(&self.docker_args, "--pids-limit")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or(HelperPlanError::InvalidLimits)?;
        if !argument_after(&self.docker_args, "--memory").is_some_and(is_valid_memory_limit)
            || !argument_after(&self.docker_args, "--cpus").is_some_and(is_valid_cpu_limit)
        {
            return Err(HelperPlanError::InvalidLimits);
        }
        let expected_env: Vec<String> = fixed_environment()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        let envs: Vec<&String> = self
            .docker_args
            .windows(2)
            .filter(|pair| pair[0] == "--env")
            .map(|pair| &pair[1])
            .collect();
        if envs.len() != expected_env.len()
            || expected_env.iter().any(|expected| {
                envs.iter()
                    .filter(|value| value.as_str() == expected)
                    .count()
                    != 1
            })
        {
            return Err(HelperPlanError::UnsafePath);
        }
        let git = self
            .docker_args
            .iter()
            .position(|argument| argument == "git")
            .ok_or(HelperPlanError::InvalidImage)?;
        if git < 2 {
            return Err(HelperPlanError::InvalidImage);
        }
        validate_pinned_image(&self.docker_args[git - 1]).map_err(|error| match error {
            ImageError::Unpinned => HelperPlanError::UnpinnedImage,
            ImageError::Invalid => HelperPlanError::InvalidImage,
        })?;
        if self.cleanup_args
            != [
                "docker".to_owned(),
                "rm".to_owned(),
                "-f".to_owned(),
                self.container_name.clone(),
            ]
        {
            return Err(HelperPlanError::InvalidCommandArgument);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn test_with_docker_args(mut self, docker_args: Vec<String>) -> Self {
        self.docker_args = docker_args;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HelperPlanError {
    InvalidLimits,
    UnpinnedImage,
    InvalidImage,
    UnsafePath,
    RootUser,
    UnsafeName,
    InvalidCommandArgument,
}

impl std::fmt::Display for HelperPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidLimits => "helper limits are invalid",
            Self::UnpinnedImage => "helper image is not digest-pinned",
            Self::InvalidImage => "helper image is invalid",
            Self::UnsafePath => "helper private path is unsafe",
            Self::RootUser => "helper user must be non-root",
            Self::UnsafeName => "helper container name is unsafe",
            Self::InvalidCommandArgument => "helper git command argument is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for HelperPlanError {}

pub(crate) fn plan_command(
    paths: &HelperPaths,
    command: &TrustedGitCommand,
    limits: &GitVerifyLimits,
    deadline: Duration,
) -> Result<HelperPlan, HelperPlanError> {
    plan_command_with_output_limit(paths, command, limits, deadline, limits.max_output_bytes)
}

pub(crate) fn plan_command_with_output_limit(
    paths: &HelperPaths,
    command: &TrustedGitCommand,
    limits: &GitVerifyLimits,
    deadline: Duration,
    output_limit: usize,
) -> Result<HelperPlan, HelperPlanError> {
    if output_limit == 0 {
        return Err(HelperPlanError::InvalidLimits);
    }
    limits.validate()?;
    validate_private_path(&paths.operation_root)?;
    validate_private_path(&paths.repo_dir)?;
    validate_private_path(&paths.home_dir)?;
    validate_private_path(&paths.template_dir)?;
    if !paths.repo_dir.starts_with(&paths.operation_root)
        || !paths.home_dir.starts_with(&paths.operation_root)
        || !paths.template_dir.starts_with(&paths.operation_root)
    {
        return Err(HelperPlanError::UnsafePath);
    }
    if paths.uid == 0 || paths.gid == 0 {
        return Err(HelperPlanError::RootUser);
    }

    let container_name = generated_container_name();
    if !is_safe_container_name(&container_name) {
        return Err(HelperPlanError::UnsafeName);
    }
    let mount_mode = MountMode::for_command(command);
    let scratch = limits.max_output_bytes;

    let mut docker_args = vec![
        "docker".to_owned(),
        "run".to_owned(),
        "--rm".to_owned(),
        "--pull=never".to_owned(),
        "--name".to_owned(),
        container_name.clone(),
        "--user".to_owned(),
        format!("{}:{}", paths.uid, paths.gid),
        "--read-only".to_owned(),
        "--tmpfs".to_owned(),
        format!("/tmp:rw,nosuid,nodev,noexec,size={scratch}"),
        "--shm-size".to_owned(),
        scratch.to_string(),
        "--ulimit".to_owned(),
        format!("fsize={scratch}"),
        "--mount".to_owned(),
        format!(
            "type=bind,source={},target={CONTAINER_OPERATION_ROOT},{}",
            paths.operation_root.display(),
            mount_mode.suffix()
        ),
        "--workdir".to_owned(),
        CONTAINER_OPERATION_ROOT.to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--security-opt".to_owned(),
        "no-new-privileges:true".to_owned(),
        "--pids-limit".to_owned(),
        limits.pids_limit.to_string(),
        "--memory".to_owned(),
        limits.memory_limit.clone(),
        "--cpus".to_owned(),
        limits.cpu_limit.clone(),
        "--network".to_owned(),
        "none".to_owned(),
    ];
    for &(name, value) in fixed_environment() {
        docker_args.push("--env".to_owned());
        docker_args.push(format!("{name}={value}"));
    }
    docker_args.push(limits.image.clone());
    docker_args.push("git".to_owned());
    docker_args.extend(git_arguments(command)?);

    Ok(HelperPlan {
        cleanup_args: vec![
            "docker".to_owned(),
            "rm".to_owned(),
            "-f".to_owned(),
            container_name.clone(),
        ],
        container_name,
        docker_args,
        deadline,
        max_output_bytes: output_limit,
        mount_mode,
        scratch_bytes: scratch,
    })
}

pub(crate) fn fixed_environment() -> &'static [(&'static str, &'static str)] {
    &[
        ("HOME", "/git-verify/home"),
        ("PATH", "/usr/bin:/bin"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_DEFAULT_REF_FORMAT", "files"),
        ("LC_ALL", "C"),
    ]
}

fn argument_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let position = args.iter().position(|argument| argument == flag)?;
    args.get(position + 1).map(String::as_str)
}

fn git_arguments(command: &TrustedGitCommand) -> Result<Vec<String>, HelperPlanError> {
    let args = match command {
        TrustedGitCommand::Init {
            object_format,
            initial_branch,
        } => {
            require_branch_argument(initial_branch)?;
            vec![
                "init".to_owned(),
                "--bare".to_owned(),
                format!("--object-format={}", object_format.git_name()),
                format!("--initial-branch={initial_branch}"),
                "--ref-format=files".to_owned(),
                format!("--template={CONTAINER_OPERATION_ROOT}/template"),
                CONTAINER_REPO_DIR.to_owned(),
            ]
        }
        TrustedGitCommand::ShowObjectFormat => vec![
            "-C".to_owned(),
            CONTAINER_REPO_DIR.to_owned(),
            "rev-parse".to_owned(),
            "--show-object-format".to_owned(),
        ],
        TrustedGitCommand::Fsck => vec![
            "-C".to_owned(),
            CONTAINER_REPO_DIR.to_owned(),
            "fsck".to_owned(),
            "--no-dangling".to_owned(),
            "--no-progress".to_owned(),
        ],
        TrustedGitCommand::CatFileCommit { head } => {
            require_hex_argument(head)?;
            vec![
                "-C".to_owned(),
                CONTAINER_REPO_DIR.to_owned(),
                "cat-file".to_owned(),
                "-e".to_owned(),
                format!("{head}^{{commit}}"),
            ]
        }
        TrustedGitCommand::MergeBaseIsAncestor { base, head } => {
            require_hex_argument(base)?;
            require_hex_argument(head)?;
            vec![
                "-C".to_owned(),
                CONTAINER_REPO_DIR.to_owned(),
                "merge-base".to_owned(),
                "--is-ancestor".to_owned(),
                base.clone(),
                head.clone(),
            ]
        }
        TrustedGitCommand::RevParseCommit { head } => {
            require_hex_argument(head)?;
            vec![
                "-C".to_owned(),
                CONTAINER_REPO_DIR.to_owned(),
                "rev-parse".to_owned(),
                format!("{head}^{{commit}}"),
            ]
        }
        TrustedGitCommand::ShowRef { branch } => {
            require_branch_argument(branch)?;
            vec![
                "-C".to_owned(),
                CONTAINER_REPO_DIR.to_owned(),
                "show-ref".to_owned(),
                "--verify".to_owned(),
                "--hash".to_owned(),
                format!("refs/heads/{branch}"),
            ]
        }
        TrustedGitCommand::BundleCreate { branch } => {
            require_branch_argument(branch)?;
            vec![
                "-C".to_owned(),
                CONTAINER_REPO_DIR.to_owned(),
                "bundle".to_owned(),
                "create".to_owned(),
                "-q".to_owned(),
                "-".to_owned(),
                format!("refs/heads/{branch}"),
            ]
        }
    };
    Ok(args)
}

pub(super) fn is_valid_branch(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.starts_with('.')
        || value.ends_with('.')
        || value.ends_with(".lock")
        || value.contains("..")
        || value.contains("@{")
        || value.contains("//")
    {
        return false;
    }
    if value.split('/').any(|component| {
        component.is_empty()
            || component == "."
            || component == ".."
            || component.starts_with('.')
            || component.ends_with('.')
            || component.ends_with(".lock")
    }) {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

fn require_branch_argument(value: &str) -> Result<(), HelperPlanError> {
    if is_valid_branch(value) {
        Ok(())
    } else {
        Err(HelperPlanError::InvalidCommandArgument)
    }
}

fn require_hex_argument(value: &str) -> Result<(), HelperPlanError> {
    if (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(HelperPlanError::InvalidCommandArgument)
    }
}

fn validate_private_path(path: &Path) -> Result<(), HelperPlanError> {
    if !path.is_absolute() {
        return Err(HelperPlanError::UnsafePath);
    }
    let Some(text) = path.to_str() else {
        return Err(HelperPlanError::UnsafePath);
    };
    if text.is_empty()
        || text.len() > 4096
        || text
            .chars()
            .any(|character| matches!(character, ',' | '=' | '\n' | '\r' | '\0'))
        || text.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(HelperPlanError::UnsafePath);
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(HelperPlanError::UnsafePath);
    }
    Ok(())
}

enum ImageError {
    Unpinned,
    Invalid,
}

fn validate_pinned_image(image: &str) -> Result<(), ImageError> {
    let Some((name, digest)) = image.split_once('@') else {
        return Err(ImageError::Unpinned);
    };
    if name.is_empty()
        || name.len() > 255
        || name.contains('@')
        || name
            .chars()
            .any(|character| matches!(character, ',' | '\n' | '\r' | '\0' | ' '))
        || name.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(ImageError::Invalid);
    }
    let Some(hash) = digest.strip_prefix("sha256:") else {
        return Err(ImageError::Invalid);
    };
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ImageError::Invalid);
    }
    Ok(())
}

fn is_valid_memory_limit(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 || value.trim() != value {
        return false;
    }
    let (number, suffix) = match value.as_bytes().last() {
        Some(byte) if byte.is_ascii_alphabetic() => value.split_at(value.len() - 1),
        _ => (value, ""),
    };
    if !matches!(
        suffix.to_ascii_lowercase().as_str(),
        "" | "b" | "k" | "m" | "g"
    ) {
        return false;
    }
    number
        .parse::<f64>()
        .map(|amount| amount.is_finite() && amount > 0.0)
        .unwrap_or(false)
}

fn is_valid_cpu_limit(value: &str) -> bool {
    if value.is_empty() || value.len() > 32 || value.trim() != value {
        return false;
    }
    value
        .parse::<f64>()
        .map(|amount| amount.is_finite() && amount > 0.0)
        .unwrap_or(false)
}

fn generated_container_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("df-git-verify-{}-{nanos:x}-{counter:x}", std::process::id())
}

fn is_safe_container_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
        && !name.starts_with('-')
        && !name.starts_with('.')
}
