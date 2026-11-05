//! Durable, credential-free resource-workspace Git sync.
//!
//! The route splits the scoped broker URL server-side, validates identity and
//! the fresh binding/lease, and then either reports a credential-free existing
//! checkout or admits one durable first-clone job for the already-observed
//! assigned manager. The scoped password never enters a job payload, sandbox
//! argument, database row, result, error, or repository configuration.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use runner_protocol::{
    CommandOutput, GIT_JOB_CREDENTIAL_USERNAME, GitJobCredential, GitJobErrorCode, GitReceiptPhase,
    GitReceiptReadRequest, RUNNER_MANAGER_HEARTBEAT_TTL_SECS, RunnerCommandRequest, RunnerOwner,
    is_canonical_branch, workspace_principal::application_principal_owner,
};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    actor::ActorContext,
    db::{
        queries::find_workspace_binding_by_id,
        types::{GitWorkspaceSyncRequest, GitWorkspaceSyncResponse, WorkspaceBindingResponse},
        workspace_git_jobs::{
            store::{
                GitJobReconcile, admit_and_insert_git_job, find_active_git_job_for_binding,
                find_git_job_by_id, reconcile_uncertain_git_job,
            },
            types::{AdmissionError, AdmissionLimits, GitJobRecord, GitJobResult, NewGitJob},
        },
    },
    error::AppError,
    runner::git_job_credentials::{
        GitJobCredentialEntry, GitJobCredentialIdentity, SplitScopedGitUrl, split_scoped_git_url,
    },
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const BOOTSTRAP_WAIT: Duration = Duration::from_secs(2);
const SANDBOX_TIMEOUT_MS: u64 = 10_000;
const SANDBOX_OUTPUT_BYTES: usize = 64 * 1024;
const LEGACY_CLEANUP_REQUIRED: &str = "legacy_git_cleanup_required";

/// Fixed credential-free preflight/status probe. It never receives a URL,
/// password, or caller-generated fragment: every branch is decided inside the
/// sandbox and only fixed tokens and non-secret Git identity are returned.
const PRESERVE_SCRIPT: &str = r#"set -eu
GIT_DIR=.git
if [ -L "$GIT_DIR" ] || [ ! -d "$GIT_DIR" ] || [ -L "$GIT_DIR/config" ] || [ ! -f "$GIT_DIR/config" ]; then
  echo SHAPE_UNSUPPORTED
  exit 0
fi
if [ -e "$GIT_DIR/commondir" ] || [ -e "$GIT_DIR/objects/info/alternates" ]; then
  echo SHAPE_UNSUPPORTED
  exit 0
fi
GIT="git --git-dir=$GIT_DIR --work-tree=. -c core.fsmonitor=false -c core.hooksPath=/dev/null -c gc.auto=0 -c core.untrackedCache=false"
if ! keys=$($GIT config --file "$GIT_DIR/config" --no-includes --name-only --list 2>/dev/null); then
  echo CONFIG_UNSUPPORTED
  exit 0
fi
case "$keys" in
  *include*) echo CONFIG_UNSUPPORTED; exit 0;;
esac
marker="smc""pw1."
for key in remote.origin.url remote.origin.pushurl; do
  values=$($GIT config --file "$GIT_DIR/config" --no-includes --get-all "$key" 2>/dev/null || true)
  case "$values" in
    *"://"*@*) echo RESIDUE; exit 0;;
    *"$marker"*) echo RESIDUE; exit 0;;
  esac
done
if ! head=$($GIT rev-parse --verify HEAD 2>/dev/null); then
  echo HEAD_UNSUPPORTED
  exit 0
fi
branch=$($GIT symbolic-ref --short -q HEAD 2>/dev/null || true)
if ! status=$($GIT status --porcelain 2>/dev/null); then
  echo STATUS_UNSUPPORTED
  exit 0
fi
echo SAFE
echo "HEAD $head"
if [ -n "$branch" ]; then
  echo "BRANCH $branch"
else
  echo DETACHED
fi
if [ -n "$status" ]; then
  echo DIRTY
else
  echo CLEAN
fi
"#;

pub(super) async fn run(
    state: &AppState,
    actor: &ActorContext,
    request: &GitWorkspaceSyncRequest,
    resource_kind: &str,
    resource_id: &str,
) -> Result<GitWorkspaceSyncResponse, AppError> {
    let code_project_id = request.code_project_id;
    let authorization_generation = request.authorization_generation;
    if code_project_id.is_nil() || authorization_generation.is_nil() {
        return Err(AppError::bad_request("invalid git sync identity"));
    }
    let path_project = Uuid::parse_str(resource_id)
        .map_err(|_| AppError::bad_request("invalid resource workspace key"))?;
    if path_project != code_project_id {
        return Err(AppError::bad_request(
            "git sync identity does not match resource path",
        ));
    }
    let branch = match request
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(branch) => {
            if !is_canonical_branch(branch) {
                return Err(AppError::bad_request("invalid git branch"));
            }
            Some(branch.to_owned())
        }
        None => None,
    };
    let split = split_scoped_git_url(request.remote_url.trim(), code_project_id)
        .ok_or_else(|| AppError::bad_request("invalid git remote url"))?;

    let Some(snapshot) = actor.workspace_binding.as_ref() else {
        return Err(AppError::unauthorized("application bearer token required"));
    };
    let fresh = find_workspace_binding_by_id(&state.db, snapshot.workspace_binding_id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::forbidden("workspace binding is not active"))?;
    validate_binding(
        &fresh,
        resource_kind,
        resource_id,
        &authorization_generation,
    )?;
    crate::actor::admit_fenced_write(Some(&fresh), actor.lease_owner.as_deref())
        .map_err(AppError::forbidden)?;
    let workspace_root = verify_derived_root(state, actor, &fresh, resource_id)?;

    match git_state(&workspace_root)? {
        GitState::Existing => {
            if !legacy_git_shape_is_usable(&workspace_root) {
                return Err(legacy_cleanup_required());
            }
            let status = probe_existing(state, actor, &workspace_root).await?;
            if request.preserve_existing {
                return Ok(preserve_status(status));
            }
            let settings = state
                .config
                .git_jobs
                .as_ref()
                .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
            let call_deadline = Instant::now() + settings.wait.min(Duration::from_secs(60));
            let branch = branch.ok_or_else(|| {
                AppError::conflict("explicit canonical branch is required for clean fast-forward")
            })?;
            if status.dirty
                || status.branch.as_deref() != Some(branch.as_str())
                || !(is_lower_hex(&status.head, 40) || is_lower_hex(&status.head, 64))
            {
                return Err(AppError::conflict(
                    "workspace must be clean, attached, and on the requested canonical branch",
                ));
            }
            if let Some(active) = find_active_git_job_for_binding(
                &state.db,
                fresh.workspace_binding_id,
                fresh.application_id,
            )
            .await
            .map_err(AppError::internal)?
            {
                return await_active(state, actor, &fresh, active, &branch, call_deadline, false)
                    .await;
            }
            start_job(
                state,
                actor,
                &fresh,
                &branch,
                &split,
                authorization_generation,
                Some(status.head),
                false,
                call_deadline,
            )
            .await
        }
        GitState::Absent => {
            let branch = branch
                .ok_or_else(|| AppError::bad_request("branch is required for the first clone"))?;
            if !is_empty_target(&workspace_root)? {
                return Err(AppError::conflict(
                    "workspace is not empty and is not a git checkout",
                ));
            }
            if let Some(active) = find_active_git_job_for_binding(
                &state.db,
                fresh.workspace_binding_id,
                fresh.application_id,
            )
            .await
            .map_err(AppError::internal)?
            {
                let settings = state
                    .config
                    .git_jobs
                    .as_ref()
                    .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
                let call_deadline = Instant::now() + settings.wait.min(Duration::from_secs(60));
                return await_active(state, actor, &fresh, active, &branch, call_deadline, true)
                    .await;
            }
            let settings = state
                .config
                .git_jobs
                .as_ref()
                .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
            let call_deadline = Instant::now() + settings.wait.min(Duration::from_secs(60));
            start_job(
                state,
                actor,
                &fresh,
                &branch,
                &split,
                authorization_generation,
                None,
                true,
                call_deadline,
            )
            .await
        }
        GitState::Unsupported => Err(legacy_cleanup_required()),
    }
}

fn validate_binding(
    fresh: &WorkspaceBindingResponse,
    resource_kind: &str,
    resource_id: &str,
    authorization_generation: &Uuid,
) -> Result<(), AppError> {
    if !fresh.is_active
        || fresh.lifecycle_state != "active"
        || fresh.resource_kind.as_deref() != Some(resource_kind)
        || fresh.resource_id.as_deref() != Some(resource_id)
        || authorization_generation.is_nil()
    {
        return Err(AppError::forbidden(
            "workspace binding does not match resource path",
        ));
    }
    if fresh.write_lease_owner.is_none() || fresh.write_lease_acquired_at.is_none() {
        return Err(AppError::forbidden(
            "workspace is read-only: no write lease held by this session. Acquire the write lease (or take it over) before running mutating commands",
        ));
    }
    Ok(())
}

fn verify_derived_root(
    state: &AppState,
    actor: &ActorContext,
    fresh: &WorkspaceBindingResponse,
    resource_id: &str,
) -> Result<PathBuf, AppError> {
    let Some(application) = actor.application.as_ref() else {
        return Err(AppError::unauthorized("application bearer token required"));
    };
    let expected = crate::workspace::resolve_resource_workspace(
        &state.config.workspace_root,
        application,
        "code_project",
        resource_id,
    )
    .map_err(AppError::internal)?;
    let root = PathBuf::from(&fresh.workspace_root);
    if root != expected {
        return Err(AppError::conflict(
            "resource workspace root is not the derived target",
        ));
    }
    Ok(root)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitState {
    Existing,
    Absent,
    Unsupported,
}

fn git_state(workspace_root: &Path) -> Result<GitState, AppError> {
    match std::fs::symlink_metadata(workspace_root.join(".git")) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(GitState::Unsupported),
        Ok(metadata) if metadata.is_dir() => Ok(GitState::Existing),
        Ok(_) => Ok(GitState::Unsupported),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(GitState::Absent),
        Err(error) => Err(AppError::internal(error.into())),
    }
}

fn legacy_git_shape_is_usable(workspace_root: &Path) -> bool {
    let git = workspace_root.join(".git");
    let Ok(git_meta) = std::fs::symlink_metadata(&git) else {
        return false;
    };
    if !git_meta.is_dir() || git_meta.file_type().is_symlink() {
        return false;
    }
    if workspace_root.join(".git/commondir").exists()
        || workspace_root.join(".git/objects/info/alternates").exists()
    {
        return false;
    }
    let Ok(config) = std::fs::symlink_metadata(workspace_root.join(".git/config")) else {
        return false;
    };
    config.is_file() && !config.file_type().is_symlink()
}

fn is_empty_target(workspace_root: &Path) -> Result<bool, AppError> {
    match std::fs::read_dir(workspace_root) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(AppError::internal(error.into())),
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PreserveStatus {
    head: String,
    branch: Option<String>,
    dirty: bool,
}

async fn probe_existing(
    state: &AppState,
    actor: &ActorContext,
    workspace_root: &Path,
) -> Result<PreserveStatus, AppError> {
    let output = run_sandbox(
        state,
        actor,
        workspace_root,
        sandbox_args(&["/bin/sh", "-c", PRESERVE_SCRIPT]),
        false,
    )
    .await?;
    if output.truncated || output.exit_code != Some(0) {
        return Err(legacy_cleanup_required());
    }
    parse_preserve_output(output.stdout.as_bytes()).ok_or_else(legacy_cleanup_required)
}

fn preserve_status(status: PreserveStatus) -> GitWorkspaceSyncResponse {
    GitWorkspaceSyncResponse {
        status: "ready".to_owned(),
        cloned: false,
        dirty: status.dirty,
        branch: status.branch,
        head_commit: Some(status.head),
    }
}

async fn await_active(
    state: &AppState,
    actor: &ActorContext,
    fresh: &WorkspaceBindingResponse,
    mut row: GitJobRecord,
    branch: &str,
    call_deadline: Instant,
    cloned: bool,
) -> Result<GitWorkspaceSyncResponse, AppError> {
    loop {
        match row.state.as_str() {
            "succeeded" => return succeeded_response(&row, branch, cloned),
            "failed" => return Err(failed_status(row.error_summary.as_deref())),
            "uncertain" => {
                return reconcile_uncertain(
                    state,
                    actor,
                    fresh,
                    &row,
                    branch,
                    call_deadline,
                    cloned,
                )
                .await;
            }
            "superseded" => return Err(AppError::conflict("git sync was superseded")),
            _ => {}
        }
        if Instant::now() >= call_deadline || row.deadline_at <= Utc::now() {
            return Err(AppError::service_unavailable(
                "git sync is still in progress",
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        row = find_git_job_by_id(&state.db, row.job_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
    }
}

fn succeeded_response(
    row: &GitJobRecord,
    branch: &str,
    cloned: bool,
) -> Result<GitWorkspaceSyncResponse, AppError> {
    let result = row
        .result
        .clone()
        .and_then(|value| serde_json::from_value::<GitJobResult>(value).ok())
        .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
    let GitJobResult::SyncSucceeded {
        head,
        base,
        published,
    } = result;
    if !row.cleanup_confirmed
        || !published
        || base != row.expected_base
        || !(is_lower_hex(&head, 40) || is_lower_hex(&head, 64))
    {
        return Err(AppError::service_unavailable("git sync is unavailable"));
    }
    Ok(GitWorkspaceSyncResponse {
        status: "ready".to_owned(),
        cloned,
        dirty: false,
        branch: Some(branch.to_owned()),
        head_commit: Some(head),
    })
}

fn parse_preserve_output(stdout: &[u8]) -> Option<PreserveStatus> {
    let text = std::str::from_utf8(stdout).ok()?;
    let mut lines = text.lines();
    if lines.next() != Some("SAFE") {
        return None;
    }
    let head = lines.next()?.strip_prefix("HEAD ")?;
    if !(is_lower_hex(head, 40) || is_lower_hex(head, 64)) {
        return None;
    }
    let branch_line = lines.next()?;
    let branch = if branch_line == "DETACHED" {
        None
    } else {
        let branch = branch_line.strip_prefix("BRANCH ")?;
        if !is_canonical_branch(branch) {
            return None;
        }
        Some(branch.to_owned())
    };
    let dirty = match lines.next()? {
        "DIRTY" => true,
        "CLEAN" => false,
        _ => return None,
    };
    if lines.next().is_some() {
        return None;
    }
    Some(PreserveStatus {
        head: head.to_owned(),
        branch,
        dirty,
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[allow(clippy::too_many_arguments)]
async fn start_job(
    state: &AppState,
    actor: &ActorContext,
    fresh: &WorkspaceBindingResponse,
    branch: &str,
    split: &SplitScopedGitUrl,
    authorization_generation: Uuid,
    expected_base: Option<String>,
    cloned: bool,
    call_deadline: Instant,
) -> Result<GitWorkspaceSyncResponse, AppError> {
    let settings = state
        .config
        .git_jobs
        .as_ref()
        .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
    let workspace_root = PathBuf::from(&fresh.workspace_root);
    let started = Utc::now();
    let lease_expires = fresh
        .write_lease_expires_at
        .filter(|expires| *expires > started)
        .ok_or_else(|| AppError::forbidden("workspace write lease has expired"))?;
    let configured_deadline = chrono::Duration::from_std(settings.deadline)
        .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?;
    let configured_wait = chrono::Duration::from_std(settings.wait)
        .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?;
    let job_deadline = (started + configured_deadline).min(lease_expires);
    let wait_deadline = (started + configured_wait).min(job_deadline);
    let initial_remaining = (wait_deadline - started)
        .to_std()
        .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?;

    let managers = assigned_managers(&state.db, fresh.workspace_binding_id).await?;
    let assigned_manager_id = match managers.as_slice() {
        [manager] => *manager,
        [] => {
            let bootstrap_budget = BOOTSTRAP_WAIT.min(initial_remaining);
            let bootstrap = tokio::time::timeout(
                bootstrap_budget,
                bootstrap_assignment(state, actor, &workspace_root),
            )
            .await
            .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?;
            bootstrap?;
            let budget = BOOTSTRAP_WAIT.min(
                (wait_deadline - Utc::now())
                    .to_std()
                    .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?,
            );
            let bootstrap_start = Instant::now();
            let reread = loop {
                let managers = assigned_managers(&state.db, fresh.workspace_binding_id).await?;
                if !managers.is_empty() || bootstrap_start.elapsed() >= budget {
                    break managers;
                }
                tokio::time::sleep(POLL_INTERVAL.min(budget)).await;
            };
            match reread.as_slice() {
                [manager] => *manager,
                _ => {
                    return Err(AppError::service_unavailable(
                        "git sync assignment is unavailable",
                    ));
                }
            }
        }
        _ => {
            return Err(AppError::service_unavailable(
                "git sync assignment is unavailable",
            ));
        }
    };

    let remaining_wait = (wait_deadline - Utc::now())
        .to_std()
        .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?;
    if remaining_wait.is_zero() {
        return Err(AppError::service_unavailable("git sync is unavailable"));
    }
    let wait = remaining_wait;

    let lease_owner = fresh
        .write_lease_owner
        .clone()
        .ok_or_else(|| AppError::forbidden("workspace write lease is missing"))?;
    let lease_acquired_at = fresh
        .write_lease_acquired_at
        .ok_or_else(|| AppError::forbidden("workspace write lease is missing"))?;
    let owner = application_principal_owner(
        fresh.application_id,
        fresh.workspace_binding_id,
        &fresh.external_user_id,
        &fresh.workspace_key,
    )
    .map_err(|_| AppError::internal(anyhow::anyhow!("invalid binding principal")))?;
    let RunnerOwner::ApplicationPrincipal {
        principal_digest, ..
    } = owner
    else {
        return Err(AppError::internal(anyhow::anyhow!(
            "binding principal is not an application principal"
        )));
    };

    let job_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let token = format!("gjt-{}", Uuid::new_v4().simple());
    let deadline_unix_ms = job_deadline.timestamp_millis();
    let identity = GitJobCredentialIdentity {
        job_id,
        operation_id,
        assigned_manager_id,
        application_id: fresh.application_id,
        workspace_binding_id: fresh.workspace_binding_id,
        principal_digest: principal_digest.clone(),
        code_project_id: fresh
            .resource_id
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| AppError::internal(anyhow::anyhow!("invalid resource identity")))?,
        authorization_generation,
        canonical_branch: branch.to_owned(),
        deadline_unix_ms,
    };
    let credential = GitJobCredential {
        username: GIT_JOB_CREDENTIAL_USERNAME.to_owned(),
        password: split.password.clone(),
        remote_url: split.remote_url.clone(),
        deadline_unix_ms,
    };
    state
        .git_job_credentials
        .insert(GitJobCredentialEntry {
            identity: identity.clone(),
            credential,
        })
        .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?;

    let job = NewGitJob {
        job_id,
        token,
        operation_id,
        application_id: fresh.application_id,
        workspace_binding_id: fresh.workspace_binding_id,
        assigned_manager_id,
        code_project_id: identity.code_project_id,
        authorization_generation,
        canonical_branch: branch.to_owned(),
        expected_base,
        lease_owner,
        lease_acquired_at,
        reserved_bytes: settings.reserved_bytes,
        deadline_at: job_deadline,
    };
    let limits = AdmissionLimits {
        max_active: settings.max_active,
        max_bytes: settings.max_bytes,
        max_per_binding: settings.max_per_binding,
        max_per_manager: settings.max_per_manager,
        max_bytes_per_manager: settings.max_bytes_per_manager,
    };
    let admitted = match admit_and_insert_git_job(&state.db, job, limits).await {
        Ok(record) => record,
        Err(error) => {
            state.git_job_credentials.remove(&identity);
            return Err(map_admission(error));
        }
    };
    if admitted.job_id != identity.job_id
        || admitted.operation_id != identity.operation_id
        || admitted.assigned_manager_id != identity.assigned_manager_id
        || admitted.application_id != identity.application_id
        || admitted.workspace_binding_id != identity.workspace_binding_id
        || admitted.code_project_id != identity.code_project_id
        || admitted.authorization_generation != identity.authorization_generation
        || admitted.canonical_branch != identity.canonical_branch
        || admitted.principal_digest != identity.principal_digest
        || admitted.deadline_at.timestamp_millis() != identity.deadline_unix_ms
    {
        state.git_job_credentials.remove(&identity);
        return Err(AppError::service_unavailable("git sync is unavailable"));
    }

    let poll_budget = wait.min(
        (job_deadline - Utc::now())
            .to_std()
            .map_err(|_| AppError::service_unavailable("git sync is unavailable"))?,
    );
    let poll_deadline = (Instant::now() + poll_budget).min(call_deadline);
    loop {
        let row = find_git_job_by_id(&state.db, job_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
        match row.state.as_str() {
            "succeeded" => {
                let result = row
                    .result
                    .clone()
                    .and_then(|value| serde_json::from_value::<GitJobResult>(value).ok())
                    .ok_or_else(|| AppError::service_unavailable("git sync is unavailable"))?;
                let GitJobResult::SyncSucceeded { head, .. } = result;
                if !row.cleanup_confirmed || !(is_lower_hex(&head, 40) || is_lower_hex(&head, 64)) {
                    return Err(AppError::service_unavailable("git sync is unavailable"));
                }
                return Ok(GitWorkspaceSyncResponse {
                    status: "ready".to_owned(),
                    cloned,
                    dirty: false,
                    branch: Some(branch.to_owned()),
                    head_commit: Some(head),
                });
            }
            "failed" => {
                return Err(failed_status(row.error_summary.as_deref()));
            }
            "uncertain" => {
                return reconcile_uncertain(
                    state,
                    actor,
                    fresh,
                    &row,
                    branch,
                    call_deadline,
                    cloned,
                )
                .await;
            }
            "superseded" => return Err(AppError::conflict("git sync was superseded")),
            _ => {}
        }
        if Instant::now() >= poll_deadline {
            return Err(AppError::service_unavailable(
                "git sync is still in progress",
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn reconcile_uncertain(
    state: &AppState,
    actor: &ActorContext,
    fresh: &WorkspaceBindingResponse,
    row: &GitJobRecord,
    branch: &str,
    call_deadline: Instant,
    cloned: bool,
) -> Result<GitWorkspaceSyncResponse, AppError> {
    let unconfirmed = || AppError::service_unavailable("git sync outcome is uncertain");
    if row.state != "uncertain"
        || row.application_id != fresh.application_id
        || row.workspace_binding_id != fresh.workspace_binding_id
        || row.canonical_branch != branch
        || row.assigned_manager_id <= 0
    {
        return Err(unconfirmed());
    }
    let project = fresh
        .resource_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(unconfirmed)?;
    if row.code_project_id != project {
        return Err(unconfirmed());
    }
    let owner = application_principal_owner(
        fresh.application_id,
        fresh.workspace_binding_id,
        &fresh.external_user_id,
        &fresh.workspace_key,
    )
    .map_err(|_| unconfirmed())?;
    let RunnerOwner::ApplicationPrincipal {
        principal_digest, ..
    } = &owner
    else {
        return Err(unconfirmed());
    };
    if row.principal_digest != *principal_digest {
        return Err(unconfirmed());
    }
    let managers = assigned_managers(&state.db, fresh.workspace_binding_id).await?;
    if !managers.contains(&row.assigned_manager_id) {
        return Err(unconfirmed());
    }
    let workspace_root = PathBuf::from(&fresh.workspace_root);
    let probe = probe_existing(state, actor, &workspace_root)
        .await
        .map_err(|_| unconfirmed())?;
    if probe.dirty
        || probe.branch.as_deref() != Some(branch)
        || !(is_lower_hex(&probe.head, 40) || is_lower_hex(&probe.head, 64))
    {
        return Err(unconfirmed());
    }
    let remaining = call_deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(10));
    if remaining.is_zero() {
        return Err(unconfirmed());
    }
    let deadline_unix_ms = Utc::now()
        .timestamp_millis()
        .saturating_add(remaining.as_millis() as i64);
    let request = GitReceiptReadRequest {
        job_id: row.job_id,
        operation_id: row.operation_id,
        assigned_manager_id: row.assigned_manager_id,
        owner,
        canonical_branch: row.canonical_branch.clone(),
        expected_base: row.expected_base.clone(),
        workspace_root: fresh.workspace_root.clone(),
        deadline_unix_ms,
    };
    request.validate().map_err(|_| unconfirmed())?;
    let ack = state
        .runner
        .read_git_receipt(request.clone())
        .await
        .map_err(|_| unconfirmed())?;
    if ack.validate_matches(&request).is_err() || !ack.confirms(&request) {
        return Err(unconfirmed());
    }
    if ack.phase != Some(GitReceiptPhase::Completed)
        || ack.base != row.expected_base
        || ack.head.as_deref() != Some(probe.head.as_str())
    {
        return Err(unconfirmed());
    }
    let reconcile = GitJobReconcile {
        job_id: row.job_id,
        token: &row.token,
        assigned_manager_id: row.assigned_manager_id,
        operation_id: row.operation_id,
        application_id: row.application_id,
        workspace_binding_id: row.workspace_binding_id,
        expected_base: row.expected_base.as_deref(),
        canonical_branch: &row.canonical_branch,
        principal_digest: &row.principal_digest,
        head: &probe.head,
    };
    let updated = reconcile_uncertain_git_job(&state.db, &reconcile)
        .await
        .map_err(AppError::internal)?;
    let current = match updated {
        Some(updated) => updated,
        None => find_git_job_by_id(&state.db, row.job_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(unconfirmed)?,
    };
    if !same_proof(&current, row, &probe.head, branch) {
        return Err(unconfirmed());
    }
    succeeded_response(&current, branch, cloned)
}

fn same_proof(current: &GitJobRecord, expected: &GitJobRecord, head: &str, branch: &str) -> bool {
    if current.state != "succeeded"
        || current.job_id != expected.job_id
        || current.token != expected.token
        || current.operation_id != expected.operation_id
        || current.application_id != expected.application_id
        || current.workspace_binding_id != expected.workspace_binding_id
        || current.assigned_manager_id != expected.assigned_manager_id
        || current.expected_base != expected.expected_base
        || current.canonical_branch != branch
        || current.principal_digest != expected.principal_digest
        || !current.cleanup_confirmed
    {
        return false;
    }
    matches!(
        current
            .result
            .clone()
            .and_then(|value| serde_json::from_value::<GitJobResult>(value).ok()),
        Some(GitJobResult::SyncSucceeded {
            head: stored_head,
            base: stored_base,
            published: true,
        }) if stored_head == head && stored_base == expected.expected_base
    )
}

#[derive(FromRow)]
struct AssignedManager {
    runner_manager_id: i64,
}

async fn assigned_managers(
    pool: &sqlx::PgPool,
    workspace_binding_id: i64,
) -> Result<Vec<i64>, AppError> {
    let rows: Vec<AssignedManager> = sqlx::query_as(include_str!(
        "../../sql/workspace_git_jobs/find_assigned_workspace_runner_manager.sql"
    ))
    .bind(workspace_binding_id)
    .bind(RUNNER_MANAGER_HEARTBEAT_TTL_SECS as f64)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::internal(error.into()))?;
    Ok(rows.into_iter().map(|row| row.runner_manager_id).collect())
}

async fn bootstrap_assignment(
    state: &AppState,
    actor: &ActorContext,
    workspace_root: &Path,
) -> Result<(), AppError> {
    let output = run_sandbox(
        state,
        actor,
        workspace_root,
        sandbox_args(&["git", "--version"]),
        false,
    )
    .await?;
    if output.exit_code != Some(0) {
        return Err(AppError::service_unavailable(
            "git sync assignment is unavailable",
        ));
    }
    Ok(())
}

fn sandbox_args(command: &[&str]) -> Vec<String> {
    [
        "-i",
        "PATH=/usr/bin:/bin",
        "HOME=/nonexistent",
        "GIT_CONFIG_NOSYSTEM=1",
        "GIT_CONFIG_GLOBAL=/dev/null",
        "GIT_OPTIONAL_LOCKS=0",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(command.iter().map(|value| (*value).to_owned()))
    .collect()
}

async fn run_sandbox(
    state: &AppState,
    actor: &ActorContext,
    workspace_root: &Path,
    args: Vec<String>,
    network_enabled: bool,
) -> Result<CommandOutput, AppError> {
    state
        .runner
        .run_command(RunnerCommandRequest {
            owner: actor.runner_owner(),
            workspace_root: workspace_root.to_path_buf(),
            working_dir: workspace_root.to_path_buf(),
            program: "/usr/bin/env".to_owned(),
            args,
            timeout_ms: Some(SANDBOX_TIMEOUT_MS),
            max_output_bytes: Some(SANDBOX_OUTPUT_BYTES),
            network_enabled,
        })
        .await
        .map_err(|_| AppError::service_unavailable("workspace runner is unavailable"))
}

fn failed_status(summary: Option<&str>) -> AppError {
    match summary {
        Some(summary)
            if summary == GitJobErrorCode::Unavailable.summary()
                || summary == GitJobErrorCode::CredentialUnavailable.summary()
                || summary == GitJobErrorCode::Timeout.summary()
                || summary == GitJobErrorCode::ResourceLimit.summary() =>
        {
            AppError::service_unavailable("git sync is unavailable")
        }
        Some(summary)
            if summary == GitJobErrorCode::WorkspaceBusy.summary()
                || summary == GitJobErrorCode::Conflict.summary()
                || summary == GitJobErrorCode::Cancelled.summary() =>
        {
            AppError::conflict("git sync failed")
        }
        Some(summary) if summary == GitJobErrorCode::ExecutionFailed.summary() => {
            AppError::service_unavailable("git sync is unavailable")
        }
        _ => AppError::service_unavailable("git sync is unavailable"),
    }
}

fn map_admission(error: AdmissionError) -> AppError {
    match error {
        AdmissionError::BindingBusy | AdmissionError::NativeTaskHolds => {
            AppError::conflict("workspace is busy")
        }
        AdmissionError::CapacityExceeded | AdmissionError::Storage(_) => {
            AppError::service_unavailable("git sync is unavailable")
        }
        AdmissionError::BindingUnavailable
        | AdmissionError::ResourceMismatch
        | AdmissionError::LeaseMismatch
        | AdmissionError::LeaseExpired
        | AdmissionError::DeadlineInvalid
        | AdmissionError::InvalidInput => AppError::conflict("git sync admission failed"),
    }
}

fn legacy_cleanup_required() -> AppError {
    AppError::conflict(LEGACY_CLEANUP_REQUIRED)
}

#[cfg(test)]
mod tests {
    use super::parse_preserve_output;

    #[test]
    fn preserve_output_accepts_only_the_closed_fixed_shape() {
        let head = "0123456789abcdef0123456789abcdef01234567";
        let clean = format!("SAFE\nHEAD {head}\nBRANCH main\nCLEAN\n");
        let parsed = parse_preserve_output(clean.as_bytes()).expect("clean");
        assert_eq!(parsed.head, head);
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert!(!parsed.dirty);

        let dirty = format!("SAFE\nHEAD {head}\nDETACHED\nDIRTY\n");
        let parsed = parse_preserve_output(dirty.as_bytes()).expect("dirty");
        assert!(parsed.branch.is_none());
        assert!(parsed.dirty);

        for bad in [
            "RESIDUE\n",
            &format!("SAFE\nHEAD {head}\nBRANCH main\nCLEAN\nextra\n"),
            &format!("SAFE\nHEAD {}\nBRANCH main\nCLEAN\n", "A".repeat(40)),
            &format!("SAFE\nHEAD {head}\nBRANCH main..bad\nCLEAN\n"),
        ] {
            assert!(parse_preserve_output(bad.as_bytes()).is_none());
        }
    }
}
