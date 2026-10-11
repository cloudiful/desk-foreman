use anyhow::bail;
use runner_protocol::{
    EnsureExecutionEnvironmentAck, NativeTaskRuntimeIdentity, StartNativeTaskRequest,
    StartNativeTaskResult, execution_environment as protocol,
};

use super::super::journal::{self, Journal};

pub(super) fn expected_journal(
    task: &NativeTaskRuntimeIdentity,
    environment_id: &str,
    generation: i64,
) -> Journal {
    Journal {
        version: journal::JOURNAL_VERSION,
        phase: journal::JournalPhase::Prepared,
        environment_id: environment_id.to_string(),
        generation,
        task_id: task.task_id,
        workspace_binding_id: task.workspace_binding_id,
        manifest_sha256: task.manifest_sha256.clone(),
        file_count: 0,
        total_bytes: 0,
    }
}

pub(super) fn validate_task_identity(
    task: &NativeTaskRuntimeIdentity,
    generation: i64,
) -> anyhow::Result<()> {
    if task.preparation_generation != generation || task.workspace_binding_id <= 0 {
        bail!("native task start identity does not match its environment generation");
    }
    if task.manifest_sha256.len() != 64
        || !task
            .manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("native task start manifest identity is invalid");
    }
    Ok(())
}

pub(super) fn start_result(
    request: &StartNativeTaskRequest,
    container_name: String,
) -> StartNativeTaskResult {
    StartNativeTaskResult {
        task: request.task.clone(),
        environment: EnsureExecutionEnvironmentAck {
            owner: request.owner.clone(),
            container_name,
            state: protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE.to_string(),
        },
    }
}
