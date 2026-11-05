//! Isolated Git clone preparation (C1).
//!
//! This component claims a secret-free job identity, fetches a transient
//! credential from the authenticated control plane, proves the per-operation
//! mount mapping, fetches into a capped tmpfs volume, verifies read-only
//! metadata, creates one bounded bundle, and returns it. It never imports
//! fetched objects into a host repository, never contacts the model workspace,
//! and never writes a job result or database row.

#[path = "git_operation/credential.rs"]
mod credential;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use desk_foreman::runner::RunnerService;
use runner_protocol::{GitJobErrorCode, GitJobOutcome, GitJobSpec, RunnerJob, RunnerJobResult};
use tokio::sync::watch;

use crate::config::{RunnerManagerConfig, SharedRunnerManagerConfig};
use crate::runtime::git_authority::docker_runner::{DockerCliExecutor, GitPlanRunner};
use crate::runtime::git_authority::install::{InstallFailureCode, install_verified_bundle};

pub(crate) use self::credential::ControlPlaneCredentialSource;
#[cfg(test)]
pub(crate) use self::credential::GitCredentialFuture;
pub(crate) use self::credential::GitCredentialSource;
use self::credential::{build_askpass_script, write_private_file};
use crate::runtime::git_authority::clone_bundle::{GitBundleIdentity, VerifiedGitBundle};
use crate::runtime::git_authority::clone_fetch::{
    parse_ls_remote, parse_object_batch, parse_tree_records,
};
use crate::runtime::git_authority::clone_plan::{
    DIAGNOSTICS_LIMIT_BYTES, GitCloneCommand, GitCloneConfig, GitCloneExecutor,
    GitCloneExecutorError, GitCloneProfile, GitCloneUnavailableReason, GitPlan, GitPlanOutcome,
    MAX_BUNDLE_CEILING_BYTES,
};
use crate::runtime::git_authority::clone_volume::{
    GitVolumePlan, create_stdout_matches, is_missing_volume_error,
};
use crate::runtime::git_authority::clone_workspace::{CloneWorkspace, MOUNT_PROOF_BLOB_SHA1};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitCloneFailureCode {
    InvalidSpec,
    InvalidConfig,
    ResourceLimit,
    Timeout,
    Cancelled,
    CredentialUnavailable,
    ProofFailed,
    VolumeFailed,
    FetchFailed,
    Conflict,
    VerificationFailed,
    BundleFailed,
    CleanupFailed,
    Retained,
}

impl std::fmt::Display for GitCloneFailureCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSpec => "git clone spec is invalid",
            Self::InvalidConfig => "git clone configuration is unavailable",
            Self::ResourceLimit => "git clone resource limit was exceeded",
            Self::Timeout => "git clone timed out",
            Self::Cancelled => "git clone was cancelled",
            Self::CredentialUnavailable => "git credential is unavailable",
            Self::ProofFailed => "git clone mount proof failed",
            Self::VolumeFailed => "git clone volume failed",
            Self::FetchFailed => "git clone fetch failed",
            Self::Conflict => "git clone branch changed",
            Self::VerificationFailed => "git clone verification failed",
            Self::BundleFailed => "git clone bundle failed",
            Self::CleanupFailed => "git clone cleanup failed",
            Self::Retained => "git clone retained private resources",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RetainedGitCloneResources {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) operation_id: uuid::Uuid,
    pub(crate) operation_dir: PathBuf,
    pub(crate) volume_name: Option<String>,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct GitCloneFailure {
    pub(crate) code: GitCloneFailureCode,
    pub(crate) retained: bool,
    resources: Option<RetainedGitCloneResources>,
}

impl GitCloneFailure {
    pub(crate) fn clean(code: GitCloneFailureCode) -> Self {
        Self {
            code,
            retained: false,
            resources: None,
        }
    }

    fn retained(code: GitCloneFailureCode, resources: RetainedGitCloneResources) -> Self {
        Self {
            code,
            retained: true,
            resources: Some(resources),
        }
    }

    #[cfg(test)]
    pub(crate) fn retained_resources(&self) -> Option<&RetainedGitCloneResources> {
        self.resources.as_ref()
    }
}

impl std::fmt::Debug for GitCloneFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitCloneFailure")
            .field("code", &self.code)
            .field("retained", &self.retained)
            .finish()
    }
}

impl std::fmt::Display for GitCloneFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.code.to_string())
    }
}

impl std::error::Error for GitCloneFailure {}

/// Prepare one identity-bound bundle from a claimed job spec.
pub(crate) async fn prepare_git_clone(
    spec: &GitJobSpec,
    config: &RunnerManagerConfig,
    credentials: &dyn GitCredentialSource,
    executor: &dyn GitCloneExecutor,
    cancel: watch::Receiver<bool>,
) -> Result<VerifiedGitBundle, GitCloneFailure> {
    spec.validate()
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::InvalidSpec))?;
    let clone_config = GitCloneConfig::from_runner(config).map_err(map_unavailable)?;
    let reserved = u64::try_from(spec.reserved_bytes)
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::ResourceLimit))?;
    let required = clone_config
        .required_reservation()
        .ok_or_else(|| GitCloneFailure::clean(GitCloneFailureCode::ResourceLimit))?;
    if reserved < required {
        return Err(GitCloneFailure::clean(GitCloneFailureCode::ResourceLimit));
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    if spec.deadline_unix_ms <= now_ms {
        return Err(GitCloneFailure::clean(GitCloneFailureCode::Timeout));
    }
    let remaining_ms = (spec.deadline_unix_ms - now_ms) as u64;
    let deadline =
        Instant::now() + Duration::from_millis(remaining_ms).min(clone_config.limits.deadline);
    if *cancel.borrow() {
        return Err(GitCloneFailure::clean(GitCloneFailureCode::Cancelled));
    }

    let workspace = CloneWorkspace::create(&clone_config, spec.job_id, spec.operation_id)
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::InvalidConfig))?;
    macro_rules! step {
        ($expr:expr, $code:expr) => {
            match $expr {
                Ok(value) => value,
                Err(_) => {
                    let resources = retained_resources(&workspace, spec, None);
                    return Err(match workspace.cleanup() {
                        Ok(()) => GitCloneFailure::clean($code),
                        Err(_) => {
                            GitCloneFailure::retained(GitCloneFailureCode::Retained, resources)
                        }
                    });
                }
            }
        };
    }

    // 1. Mount proof before volume creation, Init, or any secret-bearing helper.
    let proof_name = workspace.proof_name().to_owned();
    let proof_plan = step!(
        plan_for(
            GitCloneCommand::Proof {
                proof_name: proof_name.clone(),
            },
            &workspace,
            &clone_config,
            format!("dfgit-proof-{}", short_id(spec.operation_id)),
            256,
            DIAGNOSTICS_LIMIT_BYTES,
            deadline,
        ),
        GitCloneFailureCode::ProofFailed
    );
    let proof = step!(
        run_plan(executor, proof_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::ProofFailed
    );
    if proof.exit_code != 0
        || String::from_utf8_lossy(&proof.stdout).trim() != MOUNT_PROOF_BLOB_SHA1
    {
        return Err(fail_without_volume(
            workspace,
            spec,
            GitCloneFailureCode::ProofFailed,
        ));
    }
    if workspace.revalidate().is_err() {
        return Err(fail_without_volume(
            workspace,
            spec,
            GitCloneFailureCode::ProofFailed,
        ));
    }

    // 2. Credential channel.
    let credential = match credentials.fetch(spec, deadline).await {
        Ok(credential) => credential,
        Err(failure) => {
            let resources = retained_resources(&workspace, spec, None);
            return Err(match workspace.cleanup() {
                Ok(()) => failure,
                Err(_) => GitCloneFailure::retained(GitCloneFailureCode::Retained, resources),
            });
        }
    };
    if credential.deadline_unix_ms > spec.deadline_unix_ms
        || credential.deadline_unix_ms <= now_ms
        || credential.username != "broker"
        || !desk_foreman::runner::git_job_credentials::validate_git_job_credential(
            &credential,
            spec.code_project_id,
        )
        || !self::credential::authority_matches(
            &credential.remote_url,
            &clone_config.limits.broker_host,
        )
    {
        return Err(fail_without_volume(
            workspace,
            spec,
            GitCloneFailureCode::CredentialUnavailable,
        ));
    }
    step!(
        write_private_file(
            &workspace.control_dir().join("askpass.sh"),
            build_askpass_script().as_bytes(),
        ),
        GitCloneFailureCode::CredentialUnavailable
    );
    step!(
        write_private_file(
            &workspace.credential_dir().join("password"),
            credential.password.as_bytes(),
        ),
        GitCloneFailureCode::CredentialUnavailable
    );

    // 3. Capped tmpfs volume.
    let volume_plan = step!(
        GitVolumePlan::generate(
            spec.job_id,
            spec.operation_id,
            clone_config.limits.fetch_volume_bytes,
            clone_config.limits.uid,
            clone_config.limits.gid,
            deadline.saturating_duration_since(Instant::now()),
        ),
        GitCloneFailureCode::VolumeFailed
    );
    let create = run_volume_create(executor, &volume_plan, cancel.clone(), deadline).await;
    let created = matches!(
        &create,
        Ok(outcome) if outcome.exit_code == 0 && create_stdout_matches(&outcome.stdout, &volume_plan.name)
    );
    if !created {
        let removal = run_volume_remove(executor, &volume_plan, deadline).await;
        let removed = match &removal {
            Ok(outcome) => {
                outcome.exit_code == 0
                    || is_missing_volume_error(
                        &String::from_utf8_lossy(&outcome.stderr),
                        &volume_plan.name,
                    )
            }
            Err(_) => false,
        };
        if !removed {
            let resources = retained_resources(&workspace, spec, Some(&volume_plan));
            let operation_dir = workspace.retain();
            return Err(GitCloneFailure::retained(
                GitCloneFailureCode::VolumeFailed,
                RetainedGitCloneResources {
                    operation_dir,
                    ..resources
                },
            ));
        }
        return Err(fail_without_volume(
            workspace,
            spec,
            GitCloneFailureCode::VolumeFailed,
        ));
    }
    let volume = volume_plan;
    let volume_ref = &volume;

    macro_rules! step_volume {
        ($expr:expr, $code:expr) => {
            match $expr {
                Ok(value) => value,
                Err(_) => {
                    return Err(fail_with_volume(
                        executor,
                        workspace,
                        spec,
                        Some(volume.clone()),
                        deadline,
                        $code,
                    )
                    .await);
                }
            }
        };
    }

    // 4. Remote head, init, fetch.
    let ls_plan = step_volume!(
        plan_for_volume(
            GitCloneCommand::NetLsRemote {
                remote_url: credential.remote_url.clone(),
                branch: spec.canonical_branch.clone(),
            },
            &workspace,
            volume_ref,
            &clone_config,
            format!("dfgit-ls-{}", short_id(spec.operation_id)),
            DIAGNOSTICS_LIMIT_BYTES,
            DIAGNOSTICS_LIMIT_BYTES,
            deadline,
        ),
        GitCloneFailureCode::FetchFailed
    );
    let ls = step_volume!(
        run_plan(executor, ls_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::FetchFailed
    );
    if ls.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::FetchFailed,
        )
        .await);
    }
    let (remote_head, object_format) = match parse_ls_remote(&ls.stdout, &spec.canonical_branch) {
        Ok(value) => value,
        Err(_) => {
            return Err(fail_with_volume(
                executor,
                workspace,
                spec,
                Some(volume.clone()),
                deadline,
                GitCloneFailureCode::FetchFailed,
            )
            .await);
        }
    };

    let init_plan = step_volume!(
        plan_for_volume(
            GitCloneCommand::FetchInit {
                object_format,
                branch: spec.canonical_branch.clone(),
            },
            &workspace,
            volume_ref,
            &clone_config,
            format!("dfgit-init-{}", short_id(spec.operation_id)),
            DIAGNOSTICS_LIMIT_BYTES,
            DIAGNOSTICS_LIMIT_BYTES,
            deadline,
        ),
        GitCloneFailureCode::FetchFailed
    );
    let init = step_volume!(
        run_plan(executor, init_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::FetchFailed
    );
    if init.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::FetchFailed,
        )
        .await);
    }

    let fetch_plan = step_volume!(
        plan_for_volume(
            GitCloneCommand::Fetch {
                remote_url: credential.remote_url.clone(),
                branch: spec.canonical_branch.clone(),
            },
            &workspace,
            volume_ref,
            &clone_config,
            format!("dfgit-fetch-{}", short_id(spec.operation_id)),
            DIAGNOSTICS_LIMIT_BYTES,
            DIAGNOSTICS_LIMIT_BYTES,
            deadline,
        ),
        GitCloneFailureCode::FetchFailed
    );
    let fetched = step_volume!(
        run_plan(executor, fetch_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::FetchFailed
    );
    if fetched.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::FetchFailed,
        )
        .await);
    }
    // Credentialed helpers have returned; remove the secret before verification.
    step_volume!(
        workspace.remove_credentials(),
        GitCloneFailureCode::CleanupFailed
    );

    // 5. Read-only verification and metadata.
    let head = remote_head;
    let local_ref_plan = step_volume!(
        verify_plan(
            GitCloneCommand::ReadLocalRef {
                branch: spec.canonical_branch.clone(),
            },
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::VerificationFailed
    );
    let local_ref = step_volume!(
        run_plan(executor, local_ref_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::VerificationFailed
    );
    if local_ref.exit_code != 0
        || crate::runtime::git_authority::verify_fetched_head(
            &local_ref.stdout,
            &spec.canonical_branch,
            &head,
        )
        .is_err()
    {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::Conflict,
        )
        .await);
    }
    let format_plan = step_volume!(
        verify_plan(
            GitCloneCommand::ShowObjectFormat,
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::VerificationFailed
    );
    let format = step_volume!(
        run_plan(executor, format_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::VerificationFailed
    );
    if format.exit_code != 0
        || String::from_utf8_lossy(&format.stdout).trim() != object_format.git_name()
    {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::VerificationFailed,
        )
        .await);
    }

    let fsck_plan = step_volume!(
        verify_plan(
            GitCloneCommand::Fsck,
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::VerificationFailed
    );
    let fsck = step_volume!(
        run_plan(executor, fsck_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::VerificationFailed
    );
    if fsck.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::VerificationFailed,
        )
        .await);
    }

    let commit_plan = step_volume!(
        verify_plan(
            GitCloneCommand::CatFileCommit { head: head.clone() },
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::VerificationFailed
    );
    let commit = step_volume!(
        run_plan(executor, commit_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::VerificationFailed
    );
    if commit.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::VerificationFailed,
        )
        .await);
    }

    let head_plan = step_volume!(
        verify_plan(
            GitCloneCommand::Verify {
                head: head.clone(),
                expected_base: spec.expected_base.clone(),
            },
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::VerificationFailed
    );
    let head_out = step_volume!(
        run_plan(executor, head_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::VerificationFailed
    );
    if head_out.exit_code != 0 || String::from_utf8_lossy(&head_out.stdout).trim() != head {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::VerificationFailed,
        )
        .await);
    }
    if let Some(base) = &spec.expected_base {
        let ancestry_plan = step_volume!(
            verify_plan(
                GitCloneCommand::MergeBaseIsAncestor {
                    base: base.clone(),
                    head: head.clone(),
                },
                &workspace,
                volume_ref,
                &clone_config,
                spec,
                deadline,
            ),
            GitCloneFailureCode::Conflict
        );
        let ancestry = step_volume!(
            run_plan(executor, ancestry_plan, cancel.clone(), deadline).await,
            GitCloneFailureCode::Conflict
        );
        if ancestry.exit_code != 0 {
            return Err(fail_with_volume(
                executor,
                workspace,
                spec,
                Some(volume.clone()),
                deadline,
                GitCloneFailureCode::Conflict,
            )
            .await);
        }
    }

    let tree_plan = step_volume!(
        verify_plan(
            GitCloneCommand::ListTreeSizes { head: head.clone() },
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::ResourceLimit
    );
    let tree_out = step_volume!(
        run_plan(executor, tree_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::ResourceLimit
    );
    if tree_out.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::VerificationFailed,
        )
        .await);
    }
    let metadata_cap = u64::try_from(spec.reserved_bytes)
        .unwrap_or(0)
        .min(MAX_BUNDLE_CEILING_BYTES);
    let tree = match parse_tree_records(
        &tree_out.stdout,
        clone_config.limits.tree_entries,
        metadata_cap,
    ) {
        Ok(tree) => tree,
        Err(_) => {
            return Err(fail_with_volume(
                executor,
                workspace,
                spec,
                Some(volume.clone()),
                deadline,
                GitCloneFailureCode::ResourceLimit,
            )
            .await);
        }
    };

    let object_plan = step_volume!(
        verify_plan(
            GitCloneCommand::ListObjectSizes,
            &workspace,
            volume_ref,
            &clone_config,
            spec,
            deadline,
        ),
        GitCloneFailureCode::ResourceLimit
    );
    let object_out = step_volume!(
        run_plan(executor, object_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::ResourceLimit
    );
    if object_out.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::VerificationFailed,
        )
        .await);
    }
    let objects = match parse_object_batch(
        &object_out.stdout,
        clone_config.limits.tree_entries,
        clone_config.limits.logical_object_bytes,
    ) {
        Ok(objects) => objects,
        Err(_) => {
            return Err(fail_with_volume(
                executor,
                workspace,
                spec,
                Some(volume.clone()),
                deadline,
                GitCloneFailureCode::ResourceLimit,
            )
            .await);
        }
    };

    // 6. One-ref bundle, bounded before host write.
    let bundle_plan = step_volume!(
        plan_for_volume(
            GitCloneCommand::BundleCreate {
                branch: spec.canonical_branch.clone(),
            },
            &workspace,
            volume_ref,
            &clone_config,
            format!("dfgit-bundle-{}", short_id(spec.operation_id)),
            usize::try_from(clone_config.limits.bundle_bytes).unwrap_or(usize::MAX),
            DIAGNOSTICS_LIMIT_BYTES,
            deadline,
        ),
        GitCloneFailureCode::BundleFailed
    );
    let bundle_out = step_volume!(
        run_plan(executor, bundle_plan, cancel.clone(), deadline).await,
        GitCloneFailureCode::BundleFailed
    );
    if bundle_out.exit_code != 0 {
        return Err(fail_with_volume(
            executor,
            workspace,
            spec,
            Some(volume.clone()),
            deadline,
            GitCloneFailureCode::BundleFailed,
        )
        .await);
    }

    // 7. Remove the volume before returning any artifact.
    if !volume_remove_ok(executor, &volume, deadline).await {
        let resources = retained_resources(&workspace, spec, Some(&volume));
        let operation_dir = workspace.retain();
        return Err(GitCloneFailure::retained(
            GitCloneFailureCode::CleanupFailed,
            RetainedGitCloneResources {
                operation_dir,
                ..resources
            },
        ));
    }

    let identity = GitBundleIdentity {
        job_id: spec.job_id,
        operation_id: spec.operation_id,
        application_id: spec.application_id,
        workspace_binding_id: spec.workspace_binding_id,
        principal_digest: spec.principal_digest.clone(),
        code_project_id: spec.code_project_id,
        authorization_generation: spec.authorization_generation,
        branch: spec.canonical_branch.clone(),
        expected_base: spec.expected_base.clone(),
        head,
        object_format,
    };
    VerifiedGitBundle::store(
        workspace,
        identity,
        &bundle_out.stdout,
        clone_config.limits.bundle_bytes,
        tree,
        objects,
    )
    .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::BundleFailed))
}

pub(crate) async fn execute_job(
    runner: &dyn RunnerService,
    config: &SharedRunnerManagerConfig,
    job: RunnerJob,
) -> RunnerJobResult {
    let job_id = job.job_id.clone();
    let Some(spec) = serde_json::from_value::<GitJobSpec>(job.payload.clone()).ok() else {
        return invalid_payload(job_id);
    };
    if spec.validate().is_err() || job_id != format!("gitjob:{}", spec.job_id) {
        return invalid_payload(job_id);
    }
    let snapshot = config.read().await.clone();
    let clone_config = match GitCloneConfig::from_runner(&snapshot) {
        Ok(config) => config,
        Err(_) => {
            return decoded_failure(
                &job_id,
                &spec.operation_id,
                GitJobErrorCode::Unavailable,
                false,
            );
        }
    };
    let credentials = match credential_source(&snapshot, &clone_config) {
        Some(source) => source,
        None => {
            return decoded_failure(
                &job_id,
                &spec.operation_id,
                GitJobErrorCode::Unavailable,
                false,
            );
        }
    };
    let executor =
        match DockerCliExecutor::new(snapshot.docker_cli.clone(), snapshot.docker_host.clone()) {
            Ok(executor) => GitPlanRunner::new(executor),
            Err(_) => {
                return decoded_failure(
                    &job_id,
                    &spec.operation_id,
                    GitJobErrorCode::Unavailable,
                    false,
                );
            }
        };
    let (_cancel_tx, cancel) = watch::channel(false);
    let outcome = execute_decoded(
        &spec,
        &snapshot,
        &clone_config,
        &credentials,
        &executor,
        runner,
        cancel,
    )
    .await;
    job_result(job_id, outcome, &spec.operation_id)
}

#[cfg(test)]
pub(crate) async fn execute_job_with(
    runner: &dyn RunnerService,
    config: &RunnerManagerConfig,
    job: RunnerJob,
    credentials: &dyn GitCredentialSource,
    executor: &dyn GitCloneExecutor,
    cancel: watch::Receiver<bool>,
) -> RunnerJobResult {
    let job_id = job.job_id.clone();
    let Some(spec) = serde_json::from_value::<GitJobSpec>(job.payload).ok() else {
        return invalid_payload(job_id);
    };
    if spec.validate().is_err() || job_id != format!("gitjob:{}", spec.job_id) {
        return invalid_payload(job_id);
    }
    let clone_config = match GitCloneConfig::from_runner(config) {
        Ok(config) => config,
        Err(_) => {
            return decoded_failure(
                &job_id,
                &spec.operation_id,
                GitJobErrorCode::Unavailable,
                false,
            );
        }
    };
    let outcome = execute_decoded(
        &spec,
        config,
        &clone_config,
        credentials,
        executor,
        runner,
        cancel,
    )
    .await;
    job_result(job_id, outcome, &spec.operation_id)
}

fn credential_source(
    config: &RunnerManagerConfig,
    clone_config: &GitCloneConfig,
) -> Option<ControlPlaneCredentialSource> {
    let base_url = config.control_plane_url.as_deref()?;
    let manager_id = config.manager_id.parse::<i64>().ok()?;
    ControlPlaneCredentialSource::new(
        base_url.to_owned(),
        manager_id,
        config.auth_token.clone(),
        clone_config.limits.broker_host.clone(),
    )
    .ok()
}

pub(crate) async fn execute_decoded(
    spec: &GitJobSpec,
    config: &RunnerManagerConfig,
    clone_config: &GitCloneConfig,
    credentials: &dyn GitCredentialSource,
    executor: &dyn GitCloneExecutor,
    runner: &dyn RunnerService,
    cancel: watch::Receiver<bool>,
) -> GitJobOutcome {
    let bundle = match prepare_git_clone(spec, config, credentials, executor, cancel.clone()).await
    {
        Ok(bundle) => bundle,
        Err(failure) => {
            return GitJobOutcome::Failed {
                operation_id: spec.operation_id,
                error_code: map_clone_error(failure.code),
                cleanup_confirmed: !failure.retained,
            };
        }
    };
    if spec.expected_base.is_some() {
        return match super::super::runtime::git_authority::update::update_verified_bundle(
            spec,
            config,
            clone_config,
            bundle,
            executor,
            runner,
            cancel,
        )
        .await
        {
            Ok(success) => GitJobOutcome::SyncSucceeded {
                operation_id: spec.operation_id,
                head: success.head,
                base: Some(success.base),
                published: true,
                cleanup_confirmed: true,
            },
            Err(failure) => GitJobOutcome::Failed {
                operation_id: spec.operation_id,
                error_code: map_update_error(failure.code),
                cleanup_confirmed: failure.cleanup_confirmed,
            },
        };
    }
    match install_verified_bundle(spec, config, clone_config, bundle, executor, runner, cancel)
        .await
    {
        Ok(success) => GitJobOutcome::SyncSucceeded {
            operation_id: spec.operation_id,
            head: success.head,
            base: spec.expected_base.clone(),
            published: true,
            cleanup_confirmed: true,
        },
        Err(failure) => GitJobOutcome::Failed {
            operation_id: spec.operation_id,
            error_code: map_install_error(failure.code),
            cleanup_confirmed: failure.cleanup_confirmed,
        },
    }
}

fn map_update_error(
    code: crate::runtime::git_authority::update::UpdateFailureCode,
) -> GitJobErrorCode {
    use crate::runtime::git_authority::update::UpdateFailureCode;
    match code {
        UpdateFailureCode::Unavailable => GitJobErrorCode::Unavailable,
        UpdateFailureCode::ResourceLimit => GitJobErrorCode::ResourceLimit,
        UpdateFailureCode::Timeout => GitJobErrorCode::Timeout,
        UpdateFailureCode::Cancelled => GitJobErrorCode::Cancelled,
        UpdateFailureCode::WorkspaceBusy => GitJobErrorCode::WorkspaceBusy,
        UpdateFailureCode::Conflict => GitJobErrorCode::Conflict,
        UpdateFailureCode::ExecutionFailed => GitJobErrorCode::ExecutionFailed,
    }
}

fn invalid_payload(job_id: String) -> RunnerJobResult {
    RunnerJobResult {
        job_id,
        ok: false,
        result: None,
        error: Some("invalid git job payload".to_owned()),
    }
}

fn decoded_failure(
    job_id: &str,
    operation_id: &uuid::Uuid,
    error_code: GitJobErrorCode,
    cleanup_confirmed: bool,
) -> RunnerJobResult {
    job_result(
        job_id.to_owned(),
        GitJobOutcome::Failed {
            operation_id: *operation_id,
            error_code,
            cleanup_confirmed,
        },
        operation_id,
    )
}

fn job_result(
    job_id: String,
    outcome: GitJobOutcome,
    operation_id: &uuid::Uuid,
) -> RunnerJobResult {
    let ok = matches!(outcome, GitJobOutcome::SyncSucceeded { .. });
    if *operation_id != outcome.operation_id() {
        return invalid_payload(job_id);
    }
    match serde_json::to_value(outcome) {
        Ok(value) => RunnerJobResult {
            job_id,
            ok,
            result: Some(value),
            error: None,
        },
        Err(_) => invalid_payload(job_id),
    }
}

fn map_clone_error(code: GitCloneFailureCode) -> GitJobErrorCode {
    match code {
        GitCloneFailureCode::InvalidSpec | GitCloneFailureCode::InvalidConfig => {
            GitJobErrorCode::Unavailable
        }
        GitCloneFailureCode::ResourceLimit => GitJobErrorCode::ResourceLimit,
        GitCloneFailureCode::Timeout => GitJobErrorCode::Timeout,
        GitCloneFailureCode::Cancelled => GitJobErrorCode::Cancelled,
        GitCloneFailureCode::CredentialUnavailable => GitJobErrorCode::CredentialUnavailable,
        GitCloneFailureCode::Conflict => GitJobErrorCode::Conflict,
        GitCloneFailureCode::ProofFailed
        | GitCloneFailureCode::VolumeFailed
        | GitCloneFailureCode::FetchFailed
        | GitCloneFailureCode::VerificationFailed
        | GitCloneFailureCode::BundleFailed
        | GitCloneFailureCode::CleanupFailed
        | GitCloneFailureCode::Retained => GitJobErrorCode::ExecutionFailed,
    }
}

fn map_install_error(code: InstallFailureCode) -> GitJobErrorCode {
    match code {
        InstallFailureCode::Unavailable => GitJobErrorCode::Unavailable,
        InstallFailureCode::ResourceLimit => GitJobErrorCode::ResourceLimit,
        InstallFailureCode::Timeout => GitJobErrorCode::Timeout,
        InstallFailureCode::Cancelled => GitJobErrorCode::Cancelled,
        InstallFailureCode::WorkspaceBusy => GitJobErrorCode::WorkspaceBusy,
        InstallFailureCode::Conflict => GitJobErrorCode::Conflict,
        InstallFailureCode::ExecutionFailed => GitJobErrorCode::ExecutionFailed,
    }
}

fn map_unavailable(_: GitCloneUnavailableReason) -> GitCloneFailure {
    GitCloneFailure::clean(GitCloneFailureCode::InvalidConfig)
}

fn fail_without_volume(
    workspace: CloneWorkspace,
    spec: &GitJobSpec,
    code: GitCloneFailureCode,
) -> GitCloneFailure {
    let resources = retained_resources(&workspace, spec, None);
    match workspace.cleanup() {
        Ok(()) => GitCloneFailure::clean(code),
        Err(_) => GitCloneFailure::retained(GitCloneFailureCode::Retained, resources),
    }
}

fn retained_resources(
    workspace: &CloneWorkspace,
    spec: &GitJobSpec,
    volume: Option<&GitVolumePlan>,
) -> RetainedGitCloneResources {
    RetainedGitCloneResources {
        job_id: spec.job_id,
        operation_id: spec.operation_id,
        operation_dir: workspace.operation_dir().to_path_buf(),
        volume_name: volume.map(|volume| volume.name.clone()),
    }
}

async fn fail_with_volume(
    executor: &dyn GitCloneExecutor,
    workspace: CloneWorkspace,
    spec: &GitJobSpec,
    volume: Option<GitVolumePlan>,
    deadline: Instant,
    code: GitCloneFailureCode,
) -> GitCloneFailure {
    if code == GitCloneFailureCode::Retained {
        let resources = retained_resources(&workspace, spec, volume.as_ref());
        let operation_dir = workspace.retain();
        return GitCloneFailure::retained(
            code,
            RetainedGitCloneResources {
                operation_dir,
                ..resources
            },
        );
    }
    let resources = retained_resources(&workspace, spec, volume.as_ref());
    let volume_failed = match &volume {
        Some(volume) => !volume_remove_ok(executor, volume, deadline).await,
        None => false,
    };
    if volume_failed {
        let operation_dir = workspace.retain();
        return GitCloneFailure::retained(
            code,
            RetainedGitCloneResources {
                operation_dir,
                ..resources
            },
        );
    }
    match workspace.cleanup() {
        Ok(()) => GitCloneFailure::clean(code),
        Err(_) => GitCloneFailure::retained(code, resources),
    }
}

async fn volume_remove_ok(
    executor: &dyn GitCloneExecutor,
    volume: &GitVolumePlan,
    deadline: Instant,
) -> bool {
    match run_volume_remove(executor, volume, deadline).await {
        Ok(outcome) => {
            outcome.exit_code == 0
                || is_missing_volume_error(&String::from_utf8_lossy(&outcome.stderr), &volume.name)
        }
        Err(_) => false,
    }
}

async fn run_plan(
    executor: &dyn GitCloneExecutor,
    plan: GitPlan,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<GitPlanOutcome, GitCloneFailureCode> {
    match tokio::time::timeout_at(deadline, executor.run_plan(&plan, Some(cancel))).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(GitCloneExecutorError::TimedOut)) => Err(GitCloneFailureCode::Timeout),
        Err(_) => Err(GitCloneFailureCode::Retained),
        Ok(Err(GitCloneExecutorError::Cancelled)) => Err(GitCloneFailureCode::Cancelled),
        Ok(Err(GitCloneExecutorError::OutputLimit)) => Err(GitCloneFailureCode::ResourceLimit),
        Ok(Err(GitCloneExecutorError::ReapTimeout)) | Ok(Err(GitCloneExecutorError::Failed)) => {
            Err(GitCloneFailureCode::Retained)
        }
    }
}

async fn run_volume_create(
    executor: &dyn GitCloneExecutor,
    plan: &GitVolumePlan,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<GitPlanOutcome, GitCloneFailureCode> {
    let _ = cancel;
    match tokio::time::timeout_at(deadline, executor.create_volume(plan)).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(GitCloneExecutorError::Cancelled)) => Err(GitCloneFailureCode::Cancelled),
        Ok(Err(GitCloneExecutorError::OutputLimit)) => Err(GitCloneFailureCode::ResourceLimit),
        Ok(Err(_)) | Err(_) => Err(GitCloneFailureCode::Retained),
    }
}

async fn run_volume_remove(
    executor: &dyn GitCloneExecutor,
    plan: &GitVolumePlan,
    deadline: Instant,
) -> Result<GitPlanOutcome, GitCloneFailureCode> {
    match tokio::time::timeout_at(deadline, executor.remove_volume(plan)).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) | Err(_) => Err(GitCloneFailureCode::Retained),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_for(
    command: GitCloneCommand,
    workspace: &CloneWorkspace,
    config: &GitCloneConfig,
    container_name: String,
    stdout: usize,
    stderr: usize,
    deadline: Instant,
) -> Result<GitPlan, GitCloneFailureCode> {
    let _ = GitCloneProfile::Proof;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(GitCloneFailureCode::Timeout);
    }
    GitPlan::new(
        command,
        &crate::runtime::git_authority::clone_plan::GitClonePaths {
            control_dir: workspace.host_control_dir().to_path_buf(),
            credential_dir: workspace.host_credential_dir().to_path_buf(),
            volume_name: String::new(),
            uid: config.limits.uid,
            gid: config.limits.gid,
        },
        &config.limits,
        container_name,
        remaining,
        stdout,
        stderr,
    )
    .map_err(|_| GitCloneFailureCode::InvalidConfig)
}

#[allow(clippy::too_many_arguments)]
fn plan_for_volume(
    command: GitCloneCommand,
    workspace: &CloneWorkspace,
    volume: &GitVolumePlan,
    config: &GitCloneConfig,
    container_name: String,
    stdout: usize,
    stderr: usize,
    deadline: Instant,
) -> Result<GitPlan, GitCloneFailureCode> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(GitCloneFailureCode::Timeout);
    }
    GitPlan::new(
        command,
        &crate::runtime::git_authority::clone_plan::GitClonePaths {
            control_dir: workspace.host_control_dir().to_path_buf(),
            credential_dir: workspace.host_credential_dir().to_path_buf(),
            volume_name: volume.name.clone(),
            uid: config.limits.uid,
            gid: config.limits.gid,
        },
        &config.limits,
        container_name,
        remaining,
        stdout,
        stderr,
    )
    .map_err(|_| GitCloneFailureCode::InvalidConfig)
}

#[allow(clippy::too_many_arguments)]
fn verify_plan(
    command: GitCloneCommand,
    workspace: &CloneWorkspace,
    volume: &GitVolumePlan,
    config: &GitCloneConfig,
    spec: &GitJobSpec,
    deadline: Instant,
) -> Result<GitPlan, GitCloneFailureCode> {
    let _ = spec;
    let metadata_cap = u64::try_from(spec.reserved_bytes)
        .unwrap_or(0)
        .min(MAX_BUNDLE_CEILING_BYTES);
    let stdout = usize::try_from(metadata_cap).unwrap_or(usize::MAX);
    plan_for_volume(
        command,
        workspace,
        volume,
        config,
        format!("dfgit-verify-{}", short_id(spec.operation_id)),
        stdout,
        DIAGNOSTICS_LIMIT_BYTES,
        deadline,
    )
}

fn short_id(id: uuid::Uuid) -> String {
    id.simple().to_string()[..12].to_string()
}
