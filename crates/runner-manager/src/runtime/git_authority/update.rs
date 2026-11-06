//! Managed-binding fast-forward publication.
//!
//! A verified private bundle is imported into the existing clean binding
//! workspace, the exact signed descendant is published with a compare-and-set
//! ref update, and the worktree is moved only through the trusted helper. Any
//! effect that cannot be confidently rolled back stays uncertain and retains
//! the verified receipt.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use desk_foreman::runner::RunnerService;
use runner_protocol::{GitJobSpec, GitReceiptPhase, RunnerOwner};
use tokio::sync::watch;
use tokio::time::Instant;
use uuid::Uuid;

use crate::config::RunnerManagerConfig;

use super::clone_bundle::VerifiedGitBundle;
use super::clone_plan::{
    DIAGNOSTICS_LIMIT_BYTES, GitCloneCommand, GitCloneConfig, GitCloneExecutor,
    GitCloneExecutorError, GitPlan, GitPlanOutcome, GitWorkspacePaths,
};
use super::receipt::{GIT_RECEIPT_VERSION, GitReceipt, workspace_root_sha256, write_receipt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateFailureCode {
    Unavailable,
    ResourceLimit,
    Timeout,
    Cancelled,
    WorkspaceBusy,
    Conflict,
    ExecutionFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateFailure {
    pub(crate) code: UpdateFailureCode,
    pub(crate) cleanup_confirmed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateSuccess {
    pub(crate) base: String,
    pub(crate) head: String,
}

impl From<UpdateFailureCode> for UpdateFailure {
    fn from(code: UpdateFailureCode) -> Self {
        Self {
            code,
            cleanup_confirmed: true,
        }
    }
}

struct UpdateScope<'a> {
    executor: &'a dyn GitCloneExecutor,
    paths: &'a GitWorkspacePaths,
    clone_config: &'a GitCloneConfig,
    spec: &'a GitJobSpec,
    deadline: Instant,
    cancel: watch::Receiver<bool>,
}

pub(crate) async fn update_verified_bundle(
    spec: &GitJobSpec,
    config: &RunnerManagerConfig,
    clone_config: &GitCloneConfig,
    mut bundle: VerifiedGitBundle,
    executor: &dyn GitCloneExecutor,
    runner: &dyn RunnerService,
    cancel: watch::Receiver<bool>,
) -> Result<UpdateSuccess, UpdateFailure> {
    let deadline = match operation_deadline(spec, clone_config) {
        Ok(deadline) => deadline,
        Err(code) => return Err(finish(bundle, executor, cancellable_failure(code), false).await),
    };
    let Some(base) = spec.expected_base.clone() else {
        return Err(finish(bundle, executor, UpdateFailureCode::Unavailable, false).await);
    };
    if !is_oid(&base) || preflight(spec, config).is_err() {
        return Err(finish(bundle, executor, UpdateFailureCode::Conflict, false).await);
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Some(_fence) = runner
        .acquire_git_install_fence(spec.workspace_binding_id, remaining)
        .await
    else {
        return Err(finish(bundle, executor, UpdateFailureCode::WorkspaceBusy, false).await);
    };
    if *cancel.borrow() {
        return Err(finish(bundle, executor, UpdateFailureCode::Cancelled, false).await);
    }
    let mount = match bundle.mount_stat() {
        Ok(mount) => mount,
        Err(_) => {
            return Err(finish(bundle, executor, UpdateFailureCode::Unavailable, false).await);
        }
    };
    let paths = GitWorkspacePaths {
        workspace_path: PathBuf::from(&spec.workspace_root),
        bundle_path: Some(mount.source_path.clone()),
        uid: clone_config.limits.uid,
        gid: clone_config.limits.gid,
    };
    let head = bundle.identity().head.clone();
    let mut index = 0u32;
    let scope = UpdateScope {
        executor,
        paths: &paths,
        clone_config,
        spec,
        deadline,
        cancel,
    };

    let inspect = inspect_workspace(&scope, &mut bundle, &mut index).await;
    let (current_head, current_branch, clean) = match inspect {
        Ok(values) => values,
        Err(code) => return Err(finish(bundle, executor, code, false).await),
    };
    if current_head != base || current_branch != scope.spec.canonical_branch || !clean {
        return Err(finish(bundle, executor, UpdateFailureCode::Conflict, false).await);
    }

    let verified = receipt(
        scope.spec,
        config,
        GitReceiptPhase::Verified,
        base.clone(),
        head.clone(),
        false,
        false,
    );
    if write_receipt(&scope.clone_config.private_root, &verified).is_err() {
        return Err(finish(bundle, executor, UpdateFailureCode::ExecutionFailed, false).await);
    }

    // Import only the already-verified bundle objects. No refs are written.
    if let Err(code) = run_command(
        executor,
        &mut bundle,
        &plan(
            GitCloneCommand::WorkspaceImportBundle,
            scope.paths,
            scope.clone_config,
            scope.spec.operation_id,
            &mut index,
            scope.deadline,
        )?,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await
    {
        return Err(finish(bundle, executor, code, false).await);
    }
    if let Err(code) = run_command(
        executor,
        &mut bundle,
        &plan(
            GitCloneCommand::WorkspaceCommitExists { head: head.clone() },
            scope.paths,
            scope.clone_config,
            scope.spec.operation_id,
            &mut index,
            scope.deadline,
        )?,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await
    {
        return Err(finish(bundle, executor, code, false).await);
    }
    if run_command(
        executor,
        &mut bundle,
        &plan(
            GitCloneCommand::WorkspaceIsAncestor {
                base: base.clone(),
                head: head.clone(),
            },
            scope.paths,
            scope.clone_config,
            scope.spec.operation_id,
            &mut index,
            scope.deadline,
        )?,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await
    .is_err()
    {
        return Err(finish(bundle, executor, UpdateFailureCode::Conflict, false).await);
    }

    // First worktree/ref effect. A failure after this point rolls back or
    // becomes uncertain; it never reports a clean conflict.
    let read_tree = plan(
        GitCloneCommand::WorkspaceReadTree {
            old: base.clone(),
            new: head.clone(),
        },
        scope.paths,
        scope.clone_config,
        scope.spec.operation_id,
        &mut index,
        scope.deadline,
    )?;
    if let Err(code) = run_command(
        executor,
        &mut bundle,
        &read_tree,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await
    {
        return Err(rollback(&scope, bundle, &mut index, &base, &head, code).await);
    }
    let cas = plan(
        GitCloneCommand::WorkspaceUpdateRef {
            branch: scope.spec.canonical_branch.clone(),
            old: base.clone(),
            new: head.clone(),
        },
        scope.paths,
        scope.clone_config,
        scope.spec.operation_id,
        &mut index,
        scope.deadline,
    )?;
    if let Err(code) = run_command(
        executor,
        &mut bundle,
        &cas,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await
    {
        return Err(rollback(&scope, bundle, &mut index, &base, &head, code).await);
    }
    let published = inspect_workspace(&scope, &mut bundle, &mut index).await;
    if !matches!(&published, Ok((published_head, published_branch, true)) if published_head == &head && published_branch == &scope.spec.canonical_branch)
    {
        return Err(rollback(
            &scope,
            bundle,
            &mut index,
            &base,
            &head,
            UpdateFailureCode::Conflict,
        )
        .await);
    }

    let cleanup_confirmed = bundle.cleanup().is_ok();
    if !cleanup_confirmed {
        return Err(UpdateFailure {
            code: UpdateFailureCode::ExecutionFailed,
            cleanup_confirmed: false,
        });
    }
    let completed = receipt(
        scope.spec,
        config,
        GitReceiptPhase::Completed,
        base,
        head.clone(),
        true,
        true,
    );
    if write_receipt(&scope.clone_config.private_root, &completed).is_err() {
        return Err(UpdateFailure {
            code: UpdateFailureCode::ExecutionFailed,
            cleanup_confirmed: false,
        });
    }
    Ok(UpdateSuccess {
        base: completed.base.clone().unwrap_or_default(),
        head,
    })
}

fn receipt(
    spec: &GitJobSpec,
    _config: &RunnerManagerConfig,
    phase: GitReceiptPhase,
    base: String,
    head: String,
    published: bool,
    cleanup_confirmed: bool,
) -> GitReceipt {
    GitReceipt {
        version: GIT_RECEIPT_VERSION,
        job_id: spec.job_id,
        operation_id: spec.operation_id,
        owner: spec.owner.clone(),
        canonical_branch: spec.canonical_branch.clone(),
        base: Some(base),
        head,
        workspace_root_sha256: workspace_root_sha256(&spec.workspace_root),
        phase,
        published,
        cleanup_confirmed,
    }
}

fn preflight(spec: &GitJobSpec, config: &RunnerManagerConfig) -> Result<(), ()> {
    let root = Path::new(&spec.workspace_root);
    if !root.is_absolute() {
        return Err(());
    }
    let canonical = std::fs::canonicalize(root).map_err(|_| ())?;
    if canonical != root {
        return Err(());
    }
    let manager_root = std::fs::canonicalize(&config.workspace_root).map_err(|_| ())?;
    if !canonical.starts_with(manager_root) {
        return Err(());
    }
    let root_meta = std::fs::symlink_metadata(root).map_err(|_| ())?;
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Err(());
    }
    let git = root.join(".git");
    let git_meta = std::fs::symlink_metadata(&git).map_err(|_| ())?;
    if !git_meta.is_dir() || git_meta.file_type().is_symlink() {
        return Err(());
    }
    for unsafe_path in [".git/commondir", ".git/objects/info/alternates"] {
        if std::fs::symlink_metadata(root.join(unsafe_path)).is_ok() {
            return Err(());
        }
    }
    for required in [".git/HEAD", ".git/index", ".git/objects", ".git/refs"] {
        if let Ok(meta) = std::fs::symlink_metadata(root.join(required))
            && meta.file_type().is_symlink()
        {
            return Err(());
        }
    }
    let config_path = git.join("config");
    let config_meta = std::fs::symlink_metadata(&config_path).map_err(|_| ())?;
    if !config_meta.is_file()
        || config_meta.file_type().is_symlink()
        || config_meta.uid() != current_euid()
    {
        return Err(());
    }
    let text = std::fs::read_to_string(&config_path).map_err(|_| ())?;
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("[include")
        || lowered.contains("[credential")
        || lowered.contains("insteadof")
        || lowered.contains("worktree")
        || lowered.contains("alternates")
    {
        return Err(());
    }
    Ok(())
}

async fn rollback(
    scope: &UpdateScope<'_>,
    mut bundle: VerifiedGitBundle,
    index: &mut u32,
    base: &str,
    head: &str,
    original: UpdateFailureCode,
) -> UpdateFailure {
    let rollback = plan(
        GitCloneCommand::WorkspaceReadTree {
            old: head.to_owned(),
            new: base.to_owned(),
        },
        scope.paths,
        scope.clone_config,
        scope.spec.operation_id,
        index,
        scope.deadline,
    );
    let ok = match rollback {
        Ok(plan) => run_command(
            scope.executor,
            &mut bundle,
            &plan,
            scope.cancel.clone(),
            scope.deadline,
        )
        .await
        .is_ok(),
        Err(_) => false,
    };
    let confirmed = ok
        && matches!(
            inspect_workspace(scope, &mut bundle, index).await,
            Ok((ref current, ref branch, true)) if current == base && branch == &scope.spec.canonical_branch
        );
    finish(
        bundle,
        scope.executor,
        if confirmed {
            original
        } else {
            UpdateFailureCode::ExecutionFailed
        },
        confirmed,
    )
    .await
}

async fn finish(
    bundle: VerifiedGitBundle,
    _executor: &dyn GitCloneExecutor,
    code: UpdateFailureCode,
    cleanup_confirmed: bool,
) -> UpdateFailure {
    let cleanup = bundle.cleanup().is_ok();
    UpdateFailure {
        code,
        cleanup_confirmed: cleanup_confirmed && cleanup,
    }
}

async fn inspect_workspace(
    scope: &UpdateScope<'_>,
    bundle: &mut VerifiedGitBundle,
    index: &mut u32,
) -> Result<(String, String, bool), UpdateFailureCode> {
    let head = run_command(
        scope.executor,
        bundle,
        &plan(
            GitCloneCommand::WorkspaceHead,
            scope.paths,
            scope.clone_config,
            scope.spec.operation_id,
            index,
            scope.deadline,
        )?,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await?;
    let branch = run_command(
        scope.executor,
        bundle,
        &plan(
            GitCloneCommand::WorkspaceBranch,
            scope.paths,
            scope.clone_config,
            scope.spec.operation_id,
            index,
            scope.deadline,
        )?,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await?;
    let status = run_command(
        scope.executor,
        bundle,
        &plan(
            GitCloneCommand::WorkspaceStatus,
            scope.paths,
            scope.clone_config,
            scope.spec.operation_id,
            index,
            scope.deadline,
        )?,
        scope.cancel.clone(),
        scope.deadline,
    )
    .await?;
    Ok((
        String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        String::from_utf8_lossy(&branch.stdout).trim().to_owned(),
        status.stdout.iter().all(u8::is_ascii_whitespace),
    ))
}

fn plan(
    command: GitCloneCommand,
    paths: &GitWorkspacePaths,
    clone_config: &GitCloneConfig,
    operation_id: Uuid,
    index: &mut u32,
    deadline: Instant,
) -> Result<GitPlan, UpdateFailureCode> {
    *index = index.saturating_add(1);
    GitPlan::new_workspace(
        command,
        paths,
        &clone_config.limits,
        format!("dfgit-update-{}-{index}", short_id(operation_id)),
        deadline.saturating_duration_since(Instant::now()),
        DIAGNOSTICS_LIMIT_BYTES,
        DIAGNOSTICS_LIMIT_BYTES,
    )
    .map_err(|_| UpdateFailureCode::ExecutionFailed)
}

async fn run_command(
    executor: &dyn GitCloneExecutor,
    bundle: &mut VerifiedGitBundle,
    plan: &GitPlan,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<GitPlanOutcome, UpdateFailureCode> {
    if bundle.revalidate_mount().is_err() {
        return Err(UpdateFailureCode::ExecutionFailed);
    }
    bundle.mark_busy();
    match tokio::time::timeout_at(deadline, executor.run_plan(plan, Some(cancel))).await {
        Ok(Ok(outcome)) if outcome.exit_code == 0 => {
            bundle.mark_quiescent();
            Ok(outcome)
        }
        Ok(Ok(_)) => {
            bundle.mark_quiescent();
            Err(UpdateFailureCode::Conflict)
        }
        Ok(Err(GitCloneExecutorError::TimedOut)) => {
            bundle.mark_quiescent();
            Err(UpdateFailureCode::Timeout)
        }
        Ok(Err(GitCloneExecutorError::Cancelled)) => {
            bundle.mark_quiescent();
            Err(UpdateFailureCode::Cancelled)
        }
        Ok(Err(GitCloneExecutorError::OutputLimit)) => {
            bundle.mark_quiescent();
            Err(UpdateFailureCode::ResourceLimit)
        }
        Ok(Err(GitCloneExecutorError::ReapTimeout))
        | Ok(Err(GitCloneExecutorError::Failed))
        | Err(_) => Err(UpdateFailureCode::ExecutionFailed),
    }
}

fn cancellable_failure(code: UpdateFailureCode) -> UpdateFailureCode {
    code
}

fn operation_deadline(
    spec: &GitJobSpec,
    clone_config: &GitCloneConfig,
) -> Result<Instant, UpdateFailureCode> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    if spec.deadline_unix_ms <= now_ms {
        return Err(UpdateFailureCode::Timeout);
    }
    let remaining = Duration::from_millis((spec.deadline_unix_ms - now_ms) as u64)
        .min(clone_config.limits.deadline);
    Ok(Instant::now() + remaining)
}

fn is_oid(value: &str) -> bool {
    let len = value.len();
    (len == 40 || len == 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn short_id(id: Uuid) -> String {
    id.simple().to_string()[..12].to_string()
}

/// Read-only publication inspection used by the receipt reader.
pub(crate) async fn inspect_published(
    workspace_root: &str,
    branch: &str,
    expected_head: &str,
    clone_config: &GitCloneConfig,
    executor: &dyn GitCloneExecutor,
    operation_id: Uuid,
    deadline: Instant,
) -> Option<(String, String, bool)> {
    let paths = GitWorkspacePaths {
        workspace_path: PathBuf::from(workspace_root),
        bundle_path: None,
        uid: clone_config.limits.uid,
        gid: clone_config.limits.gid,
    };
    let mut index = 0;
    let head = run_readonly(
        executor,
        &paths,
        clone_config,
        operation_id,
        &mut index,
        deadline,
        GitCloneCommand::WorkspaceHead,
    )
    .await?;
    let actual_branch = run_readonly(
        executor,
        &paths,
        clone_config,
        operation_id,
        &mut index,
        deadline,
        GitCloneCommand::WorkspaceBranch,
    )
    .await?;
    let status = run_readonly(
        executor,
        &paths,
        clone_config,
        operation_id,
        &mut index,
        deadline,
        GitCloneCommand::WorkspaceStatus,
    )
    .await?;
    Some((
        String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        String::from_utf8_lossy(&actual_branch.stdout)
            .trim()
            .to_owned(),
        expected_head == String::from_utf8_lossy(&head.stdout).trim()
            && branch == String::from_utf8_lossy(&actual_branch.stdout).trim()
            && status.stdout.iter().all(u8::is_ascii_whitespace),
    ))
}

async fn run_readonly(
    executor: &dyn GitCloneExecutor,
    paths: &GitWorkspacePaths,
    clone_config: &GitCloneConfig,
    operation_id: Uuid,
    index: &mut u32,
    deadline: Instant,
    command: GitCloneCommand,
) -> Option<GitPlanOutcome> {
    let plan = plan(command, paths, clone_config, operation_id, index, deadline).ok()?;
    match tokio::time::timeout_at(deadline, executor.run_plan(&plan, None)).await {
        Ok(Ok(outcome)) if outcome.exit_code == 0 => Some(outcome),
        _ => None,
    }
}

fn _owner_binding(owner: &RunnerOwner) -> Option<i64> {
    match owner {
        RunnerOwner::ApplicationPrincipal {
            workspace_binding_id,
            ..
        } => Some(*workspace_binding_id),
        _ => None,
    }
}

fn current_euid() -> u32 {
    std::fs::metadata(".")
        .map(|metadata| metadata.uid())
        .unwrap_or(u32::MAX)
}
