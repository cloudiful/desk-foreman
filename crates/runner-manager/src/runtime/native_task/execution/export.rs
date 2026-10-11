mod reader;
mod roots;

#[cfg(test)]
mod capture_tests;
#[cfg(test)]
mod tests;

use anyhow::bail;
use runner_protocol::{ExportNativeTaskRequest, NativeTaskExportContent, SourceSeedSnapshotFile};

use super::identity::{expected_journal, validate_task_identity};
use crate::runtime::{
    DockerRunnerBackend,
    execution_environment::{
        ExecutionEnvironmentProfile,
        file_lock::{EnvFileLock, lock_holder_token, release_and_finish},
        tombstone::TombstoneStore,
    },
    native_task::journal::JournalPhase,
};

pub(crate) async fn export_native_task(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: ExportNativeTaskRequest,
) -> anyhow::Result<NativeTaskExportContent> {
    // All errors crossing dispatch are constant and contain no host paths.
    capture(backend, profile, &request)
        .await
        .map_err(|_| anyhow::anyhow!("native task export capture refused"))
}

async fn capture(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    request: &ExportNativeTaskRequest,
) -> anyhow::Result<NativeTaskExportContent> {
    let (environment, generation) =
        runner_protocol::execution_environment::validate_environment_owner(&request.owner)
            .map_err(anyhow::Error::msg)?;
    validate_task_identity(&request.task, generation)?;
    let selection = runner_protocol::validate_native_task_export_selection(&request.paths)
        .map_err(|_| anyhow::anyhow!("invalid export selection"))?;
    if selection.selection_sha256 != request.selection_sha256 {
        bail!("export selection digest mismatch");
    }
    let uid = backend.native_tasks.effective_uid()?;
    if uid == 0 {
        bail!("export refuses root");
    }
    let deadline = profile.operation_deadline()?;
    let environment_lock = backend.environment_runner_lock(environment);
    let _guard = tokio::time::timeout(deadline.remaining()?, environment_lock.lock())
        .await
        .map_err(|_| anyhow::anyhow!("export lock timeout"))?;
    let barrier = TombstoneStore::open_existing(profile.ensure_state_dir()?, environment)?;
    let file_lock =
        EnvFileLock::acquire(&barrier.lock_path(), lock_holder_token(&backend.manager_id))?;
    let outcome = async {
        if !barrier
            .load()?
            .stopped_generation
            .is_some_and(|stopped| stopped >= generation)
        {
            bail!("export requires durable stopped barrier");
        }
        let workspace = tokio::time::timeout(
            deadline.remaining()?,
            backend
                .native_tasks
                .lock_workspace_for_stop(environment, request.task.task_id, uid),
        )
        .await
        .map_err(|_| anyhow::anyhow!("export task lock timeout"))??;
        let expected = expected_journal(&request.task, environment, generation);
        let journal = workspace
            .stop_journal(&expected, uid)?
            .ok_or_else(|| anyhow::anyhow!("export workspace missing"))?;
        if journal.phase == JournalPhase::Preparing {
            bail!("export workspace is unpublished");
        }
        workspace.validate_running_tree(uid)?;
        let tree = workspace.export_tree();
        let roots = roots::ProtectedRoots::capture(tree, uid)?;
        let mut files = Vec::new();
        let mut missing_paths = Vec::new();
        let mut total = 0usize;
        for path in &selection.paths {
            deadline.remaining_ms()?;
            match reader::selected_text(tree, path, uid)? {
                Some(text) => {
                    total = total
                        .checked_add(text.len())
                        .ok_or_else(|| anyhow::anyhow!("export overflow"))?;
                    if total > runner_protocol::MAX_SEED_TOTAL_BYTES {
                        bail!("export aggregate too large");
                    }
                    files.push(SourceSeedSnapshotFile::new(path.clone(), text));
                }
                None => missing_paths.push(path.clone()),
            }
        }
        roots.verify(uid)?;
        workspace
            .stop_journal(&expected, uid)?
            .ok_or_else(|| anyhow::anyhow!("export journal disappeared"))?;
        let output_manifest_sha256 = runner_protocol::native_task_export_output_manifest_sha256(
            &selection,
            &files,
            &missing_paths,
        )
        .map_err(|_| anyhow::anyhow!("invalid export output"))?;
        let content = NativeTaskExportContent {
            task_id: request.task.task_id,
            stop_job_id: request.stop_job_id,
            selection_sha256: selection.selection_sha256,
            output_manifest_sha256,
            files,
            missing_paths,
        };
        let checked = runner_protocol::validate_native_task_export_content(request, &content)
            .map_err(|_| anyhow::anyhow!("invalid export output"))?;
        deadline.remaining_ms()?;
        Ok(checked.content)
    }
    .await;
    release_and_finish(file_lock, outcome)
}
