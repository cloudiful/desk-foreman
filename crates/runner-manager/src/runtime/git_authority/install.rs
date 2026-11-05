//! Bounded first-clone installation into the exact resource workspace.
//!
//! One operation owns a capped tmpfs volume, clones the verified bundle into
//! it, verifies the local repository read-only, exports one bounded tar stream,
//! validates and extracts it into a pinned sibling temporary directory, and
//! publishes that directory into the exact absent/empty resource target.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use desk_foreman::runner::RunnerService;
use runner_protocol::{GitJobSpec, GitReceiptPhase};
use tokio::sync::watch;
use uuid::Uuid;

use crate::config::RunnerManagerConfig;

use super::clone_bundle::VerifiedGitBundle;
use super::clone_plan::{
    DIAGNOSTICS_LIMIT_BYTES, GitCloneCommand, GitCloneConfig, GitCloneExecutor,
    GitCloneExecutorError, GitInstallPaths, GitPlan, GitPlanOutcome,
};
use super::clone_volume::{GitVolumePlan, create_stdout_matches, is_missing_volume_error};
use super::install_archive::{self, ArchiveLimits};
use super::install_target::{InstallTarget, InstallTargetError};
use super::receipt::{GIT_RECEIPT_VERSION, GitReceipt, workspace_root_sha256, write_receipt};

const INSTALL_METADATA_BASE: u64 = 65536;
const INSTALL_METADATA_PER_ENTRY: u64 = 1024;
const INSTALL_METADATA_PAYLOAD_CEILING: u64 = 2 * 1024 * 1024;
const INSTALL_ARCHIVE_CEILING: u64 = 128 * 1024 * 1024;
const INSTALL_RESERVATION_MARGIN: u64 = 65536;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallFailureCode {
    Unavailable,
    ResourceLimit,
    Timeout,
    Cancelled,
    WorkspaceBusy,
    Conflict,
    ExecutionFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstallFailure {
    pub(crate) code: InstallFailureCode,
    pub(crate) cleanup_confirmed: bool,
    pub(crate) retained_volume: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstallSuccess {
    pub(crate) head: String,
}

#[derive(Clone, Copy, Debug)]
struct InstallBudget {
    worktree_bytes: u64,
    git_bytes: u64,
    volume_bytes: u64,
    archive_limit: usize,
    metadata_bytes: u64,
    max_entries: u64,
}

impl InstallBudget {
    fn derive(
        clone_config: &GitCloneConfig,
        spec: &GitJobSpec,
        bundle: &VerifiedGitBundle,
    ) -> Result<Self, InstallFailureCode> {
        let metadata = clone_config
            .limits
            .tree_entries
            .checked_mul(INSTALL_METADATA_PER_ENTRY)
            .and_then(|value| value.checked_add(INSTALL_METADATA_BASE))
            .ok_or(InstallFailureCode::ResourceLimit)?;
        let worktree_bytes = clone_config.limits.checkout_bytes;
        let git_bytes = clone_config
            .limits
            .logical_object_bytes
            .checked_add(metadata)
            .ok_or(InstallFailureCode::ResourceLimit)?;
        let volume_bytes = worktree_bytes
            .checked_add(git_bytes)
            .ok_or(InstallFailureCode::ResourceLimit)?;
        let archive_limit = usize::try_from(volume_bytes.min(INSTALL_ARCHIVE_CEILING))
            .map_err(|_| InstallFailureCode::ResourceLimit)?;
        let metadata_bytes = metadata.min(INSTALL_METADATA_PAYLOAD_CEILING);
        let max_entries = clone_config
            .limits
            .tree_entries
            .checked_add(bundle.objects().objects)
            .and_then(|value| value.checked_add(4096))
            .map(|value| value.min(65536))
            .ok_or(InstallFailureCode::ResourceLimit)?;
        let required = bundle
            .bundle_bytes()
            .checked_add(volume_bytes)
            .and_then(|value| value.checked_add(INSTALL_RESERVATION_MARGIN))
            .ok_or(InstallFailureCode::ResourceLimit)?;
        let reserved =
            u64::try_from(spec.reserved_bytes).map_err(|_| InstallFailureCode::ResourceLimit)?;
        if reserved < required {
            return Err(InstallFailureCode::ResourceLimit);
        }
        Ok(Self {
            worktree_bytes,
            git_bytes,
            volume_bytes,
            archive_limit,
            metadata_bytes,
            max_entries,
        })
    }
}

pub(crate) async fn install_verified_bundle(
    spec: &GitJobSpec,
    config: &RunnerManagerConfig,
    clone_config: &GitCloneConfig,
    mut bundle: VerifiedGitBundle,
    executor: &dyn GitCloneExecutor,
    runner: &dyn RunnerService,
    cancel: watch::Receiver<bool>,
) -> Result<InstallSuccess, InstallFailure> {
    let budget = match InstallBudget::derive(clone_config, spec, &bundle) {
        Ok(value) => value,
        Err(code) => {
            return Err(finish_failure(bundle, None, None, executor, false, code).await);
        }
    };
    let deadline = match operation_deadline(spec, clone_config) {
        Ok(value) => value,
        Err(code) => {
            return Err(finish_failure(bundle, None, None, executor, false, code).await);
        }
    };
    if *cancel.borrow() {
        return Err(finish_failure(
            bundle,
            None,
            None,
            executor,
            false,
            InstallFailureCode::Cancelled,
        )
        .await);
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Some(_fence) = runner
        .acquire_git_install_fence(spec.workspace_binding_id, remaining)
        .await
    else {
        return Err(finish_failure(
            bundle,
            None,
            None,
            executor,
            false,
            InstallFailureCode::WorkspaceBusy,
        )
        .await);
    };
    let mut target = match InstallTarget::open(
        &config.workspace_root,
        spec.application_id,
        spec.code_project_id,
        &spec.workspace_root,
        clone_config.limits.uid,
        clone_config.limits.gid,
    ) {
        Ok(target) => target,
        Err(error) => {
            return Err(finish_failure(
                bundle,
                None,
                None,
                executor,
                false,
                map_target_error(error),
            )
            .await);
        }
    };
    let mount = match bundle.mount_stat() {
        Ok(mount) => mount,
        Err(_) => {
            return Err(finish_failure(
                bundle,
                Some(target),
                None,
                executor,
                false,
                InstallFailureCode::ExecutionFailed,
            )
            .await);
        }
    };
    let volume = match GitVolumePlan::generate(
        spec.job_id,
        spec.operation_id,
        budget.volume_bytes,
        clone_config.limits.uid,
        clone_config.limits.gid,
        remaining,
    ) {
        Ok(volume) => volume,
        Err(_) => {
            return Err(finish_failure(
                bundle,
                Some(target),
                None,
                executor,
                false,
                InstallFailureCode::ResourceLimit,
            )
            .await);
        }
    };
    let create = tokio::time::timeout_at(deadline, executor.create_volume(&volume)).await;
    let created = matches!(
        &create,
        Ok(Ok(outcome)) if outcome.exit_code == 0 && create_stdout_matches(&outcome.stdout, &volume.name)
    );
    if !created {
        let uncertain = !matches!(&create, Ok(Ok(_)));
        return Err(finish_failure(
            bundle,
            Some(target),
            Some(volume),
            executor,
            uncertain,
            InstallFailureCode::ExecutionFailed,
        )
        .await);
    }

    let mut install_limits = clone_config.limits.clone();
    install_limits.fetch_volume_bytes = budget.volume_bytes;
    let paths = GitInstallPaths {
        bundle_path: mount.source_path.clone(),
        volume_name: volume.name.clone(),
        uid: clone_config.limits.uid,
        gid: clone_config.limits.gid,
    };
    let mut index = 0u32;
    let plan = |command: GitCloneCommand,
                profile_id: &mut u32,
                stdout: usize|
     -> Result<GitPlan, InstallFailureCode> {
        *profile_id = profile_id.saturating_add(1);
        GitPlan::new_install(
            command,
            &paths,
            &install_limits,
            format!("dfgit-install-{}-{profile_id}", short_id(spec.operation_id)),
            deadline.saturating_duration_since(Instant::now()),
            stdout,
            DIAGNOSTICS_LIMIT_BYTES,
        )
        .map_err(|_| InstallFailureCode::ExecutionFailed)
    };

    // Clone the verified bundle into the install volume.
    let commands: Vec<(GitCloneCommand, usize)> = vec![
        (
            GitCloneCommand::InstallClone {
                branch: spec.canonical_branch.clone(),
            },
            DIAGNOSTICS_LIMIT_BYTES,
        ),
        (
            GitCloneCommand::InstallRemoveOrigin,
            DIAGNOSTICS_LIMIT_BYTES,
        ),
        (
            GitCloneCommand::InstallCheckout {
                branch: spec.canonical_branch.clone(),
            },
            DIAGNOSTICS_LIMIT_BYTES,
        ),
    ];
    for (command, stdout) in commands {
        let plan = match plan(command, &mut index, stdout) {
            Ok(plan) => plan,
            Err(code) => {
                return Err(finish_failure(
                    bundle,
                    Some(target),
                    Some(volume),
                    executor,
                    false,
                    code,
                )
                .await);
            }
        };
        match run_plan(executor, &plan, &mut bundle, cancel.clone(), deadline).await {
            Ok(outcome) if outcome.exit_code == 0 => {}
            Ok(_) => {
                return Err(finish_failure(
                    bundle,
                    Some(target),
                    Some(volume),
                    executor,
                    false,
                    InstallFailureCode::ExecutionFailed,
                )
                .await);
            }
            Err(code) => {
                return Err(finish_failure(
                    bundle,
                    Some(target),
                    Some(volume),
                    executor,
                    false,
                    code,
                )
                .await);
            }
        }
    }

    // Read-only verification before export.
    let verify_commands: Vec<(GitCloneCommand, usize)> = vec![
        (
            GitCloneCommand::InstallShowObjectFormat,
            DIAGNOSTICS_LIMIT_BYTES,
        ),
        (GitCloneCommand::InstallFsck, DIAGNOSTICS_LIMIT_BYTES),
        (
            GitCloneCommand::InstallVerifyHead {
                head: bundle.identity().head.clone(),
            },
            DIAGNOSTICS_LIMIT_BYTES,
        ),
        (
            GitCloneCommand::InstallReadBranchRef {
                branch: spec.canonical_branch.clone(),
            },
            DIAGNOSTICS_LIMIT_BYTES,
        ),
        (GitCloneCommand::InstallStatusClean, DIAGNOSTICS_LIMIT_BYTES),
    ];
    for (command, stdout) in verify_commands {
        let plan = match plan(command, &mut index, stdout) {
            Ok(plan) => plan,
            Err(code) => {
                return Err(finish_failure(
                    bundle,
                    Some(target),
                    Some(volume),
                    executor,
                    false,
                    code,
                )
                .await);
            }
        };
        let outcome = match run_plan(executor, &plan, &mut bundle, cancel.clone(), deadline).await {
            Ok(outcome) => outcome,
            Err(code) => {
                return Err(finish_failure(
                    bundle,
                    Some(target),
                    Some(volume),
                    executor,
                    false,
                    code,
                )
                .await);
            }
        };
        let valid = match &plan.command {
            GitCloneCommand::InstallShowObjectFormat => {
                String::from_utf8_lossy(&outcome.stdout).trim()
                    == bundle.identity().object_format.git_name()
            }
            GitCloneCommand::InstallFsck => outcome.exit_code == 0,
            GitCloneCommand::InstallVerifyHead { .. } => {
                outcome.exit_code == 0
                    && String::from_utf8_lossy(&outcome.stdout).trim() == bundle.identity().head
            }
            GitCloneCommand::InstallReadBranchRef { .. } => {
                outcome.exit_code == 0
                    && String::from_utf8_lossy(&outcome.stdout).trim() == bundle.identity().head
            }
            GitCloneCommand::InstallStatusClean => {
                outcome.exit_code == 0 && outcome.stdout.iter().all(u8::is_ascii_whitespace)
            }
            _ => false,
        };
        if !valid {
            return Err(finish_failure(
                bundle,
                Some(target),
                Some(volume),
                executor,
                false,
                InstallFailureCode::Conflict,
            )
            .await);
        }
    }

    // Export one bounded uncompressed tar stream.
    let export_plan = match plan(
        GitCloneCommand::InstallExport,
        &mut index,
        budget.archive_limit,
    ) {
        Ok(plan) => plan,
        Err(code) => {
            return Err(
                finish_failure(bundle, Some(target), Some(volume), executor, false, code).await,
            );
        }
    };
    let export = match run_plan(
        executor,
        &export_plan,
        &mut bundle,
        cancel.clone(),
        deadline,
    )
    .await
    {
        Ok(outcome) if outcome.exit_code == 0 => outcome,
        Ok(_) => {
            return Err(finish_failure(
                bundle,
                Some(target),
                Some(volume),
                executor,
                false,
                InstallFailureCode::ExecutionFailed,
            )
            .await);
        }
        Err(code) => {
            return Err(
                finish_failure(bundle, Some(target), Some(volume), executor, false, code).await,
            );
        }
    };
    if export.stdout.len() > budget.archive_limit {
        return Err(finish_failure(
            bundle,
            Some(target),
            Some(volume),
            executor,
            false,
            InstallFailureCode::ResourceLimit,
        )
        .await);
    }
    let archive_limits = ArchiveLimits {
        worktree_bytes: budget.worktree_bytes,
        git_bytes: budget.git_bytes,
        metadata_bytes: budget.metadata_bytes,
        max_entries: budget.max_entries,
    };
    if install_archive::extract(&export.stdout, target.temp_fd(), &archive_limits).is_err() {
        return Err(finish_failure(
            bundle,
            Some(target),
            Some(volume),
            executor,
            false,
            InstallFailureCode::ExecutionFailed,
        )
        .await);
    }
    if install_archive::inspect_git(
        target.temp_fd(),
        &spec.canonical_branch,
        &bundle.identity().head,
        bundle.identity().object_format,
        budget.metadata_bytes,
    )
    .is_err()
    {
        return Err(finish_failure(
            bundle,
            Some(target),
            Some(volume),
            executor,
            false,
            InstallFailureCode::Conflict,
        )
        .await);
    }
    // Publish proof before the target effect, so a retained private receipt
    // never advertises a completed publication it did not perform.
    let verified = install_receipt(
        spec,
        clone_config,
        GitReceiptPhase::Verified,
        bundle.identity().head.clone(),
        false,
        false,
    );
    if write_receipt(&clone_config.private_root, &verified).is_err() {
        return Err(finish_failure(
            bundle,
            Some(target),
            Some(volume),
            executor,
            false,
            InstallFailureCode::ExecutionFailed,
        )
        .await);
    }
    if let Err(error) = target.publish(&cancel, deadline) {
        return Err(finish_failure(
            bundle,
            Some(target),
            Some(volume),
            executor,
            false,
            map_target_error(error),
        )
        .await);
    }

    // Success only after every helper, volume and private resource is clean.
    let head = bundle.identity().head.clone();
    let volume_removed = volume_remove_ok(executor, &volume).await;
    let bundle_clean = bundle.cleanup().is_ok();
    let target_clean = target.cleanup().is_ok();
    if !(volume_removed && bundle_clean && target_clean) {
        return Err(InstallFailure {
            code: InstallFailureCode::ExecutionFailed,
            cleanup_confirmed: false,
            retained_volume: (!volume_removed).then(|| volume.name.clone()),
        });
    }
    let completed = install_receipt(
        spec,
        clone_config,
        GitReceiptPhase::Completed,
        head.clone(),
        true,
        true,
    );
    if write_receipt(&clone_config.private_root, &completed).is_err() {
        return Err(InstallFailure {
            code: InstallFailureCode::ExecutionFailed,
            cleanup_confirmed: false,
            retained_volume: None,
        });
    }
    Ok(InstallSuccess { head })
}

fn install_receipt(
    spec: &GitJobSpec,
    clone_config: &GitCloneConfig,
    phase: GitReceiptPhase,
    head: String,
    published: bool,
    cleanup_confirmed: bool,
) -> GitReceipt {
    let _ = clone_config;
    GitReceipt {
        version: GIT_RECEIPT_VERSION,
        job_id: spec.job_id,
        operation_id: spec.operation_id,
        owner: spec.owner.clone(),
        canonical_branch: spec.canonical_branch.clone(),
        base: spec.expected_base.clone(),
        head,
        workspace_root_sha256: workspace_root_sha256(&spec.workspace_root),
        phase,
        published,
        cleanup_confirmed,
    }
}

async fn run_plan(
    executor: &dyn GitCloneExecutor,
    plan: &GitPlan,
    bundle: &mut VerifiedGitBundle,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<GitPlanOutcome, InstallFailureCode> {
    if bundle.revalidate_mount().is_err() {
        return Err(InstallFailureCode::ExecutionFailed);
    }
    bundle.mark_busy();
    match tokio::time::timeout_at(deadline, executor.run_plan(plan, Some(cancel))).await {
        Ok(Ok(outcome)) => {
            bundle.mark_quiescent();
            Ok(outcome)
        }
        Ok(Err(GitCloneExecutorError::TimedOut)) => {
            bundle.mark_quiescent();
            Err(InstallFailureCode::Timeout)
        }
        Ok(Err(GitCloneExecutorError::Cancelled)) => {
            bundle.mark_quiescent();
            Err(InstallFailureCode::Cancelled)
        }
        Ok(Err(GitCloneExecutorError::OutputLimit)) => {
            bundle.mark_quiescent();
            Err(InstallFailureCode::ResourceLimit)
        }
        Ok(Err(GitCloneExecutorError::ReapTimeout))
        | Ok(Err(GitCloneExecutorError::Failed))
        | Err(_) => Err(InstallFailureCode::ExecutionFailed),
    }
}

async fn finish_failure(
    bundle: VerifiedGitBundle,
    target: Option<InstallTarget>,
    volume: Option<GitVolumePlan>,
    executor: &dyn GitCloneExecutor,
    uncertain: bool,
    code: InstallFailureCode,
) -> InstallFailure {
    let uncertain = uncertain || !bundle.is_quiescent();
    let retained_volume = volume.as_ref().map(|volume| volume.name.clone());
    let mut confirmed = !uncertain;
    if let Some(volume) = volume
        && (uncertain || !volume_remove_ok(executor, &volume).await)
    {
        confirmed = false;
    }
    if bundle.cleanup().is_err() {
        confirmed = false;
    }
    if let Some(target) = target
        && target.cleanup().is_err()
    {
        confirmed = false;
    }
    InstallFailure {
        code,
        cleanup_confirmed: confirmed,
        retained_volume: (!confirmed).then_some(retained_volume).flatten(),
    }
}

async fn volume_remove_ok(executor: &dyn GitCloneExecutor, volume: &GitVolumePlan) -> bool {
    let mut removal = volume.clone();
    removal.deadline = CLEANUP_TIMEOUT;
    match tokio::time::timeout(Duration::from_secs(5), executor.remove_volume(&removal)).await {
        Ok(Ok(outcome)) => {
            outcome.exit_code == 0
                || is_missing_volume_error(&String::from_utf8_lossy(&outcome.stderr), &volume.name)
        }
        _ => false,
    }
}

fn operation_deadline(
    spec: &GitJobSpec,
    clone_config: &GitCloneConfig,
) -> Result<Instant, InstallFailureCode> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    if spec.deadline_unix_ms <= now_ms {
        return Err(InstallFailureCode::Timeout);
    }
    let remaining = Duration::from_millis((spec.deadline_unix_ms - now_ms) as u64)
        .min(clone_config.limits.deadline);
    Ok(Instant::now() + remaining)
}

fn map_target_error(error: InstallTargetError) -> InstallFailureCode {
    match error {
        InstallTargetError::Timeout => InstallFailureCode::Timeout,
        InstallTargetError::Cancelled => InstallFailureCode::Cancelled,
        InstallTargetError::TargetNotEmpty => InstallFailureCode::Conflict,
        InstallTargetError::InvalidPath | InstallTargetError::UnsafeParent => {
            InstallFailureCode::Unavailable
        }
        InstallTargetError::IdentityChanged | InstallTargetError::PublishFailed => {
            InstallFailureCode::Conflict
        }
        InstallTargetError::CreateFailed | InstallTargetError::CleanupFailed => {
            InstallFailureCode::ExecutionFailed
        }
    }
}

fn short_id(id: Uuid) -> String {
    id.simple().to_string()[..12].to_string()
}
