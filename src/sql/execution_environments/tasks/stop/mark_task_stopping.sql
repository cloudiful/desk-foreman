-- Attach a task-aware stop job and retain its potential container candidate.
UPDATE execution_environment_tasks
SET state = 'stopping',
    container_generation = $4,
    stop_job_id = $5,
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND target_generation = $4
  AND state IN ('prepared', 'preparing', 'starting', 'running', 'stopping', 'uncertain')
  AND stop_job_id IS NOT DISTINCT FROM $6
RETURNING
    task_id,
    operation_token,
    environment_id,
    application_id,
    workspace_binding_id,
    subject_external_user_id,
    target_generation,
    assigned_manager_id,
    state,
    container_generation,
    prepare_job_id,
    start_job_id,
    stop_job_id,
    created_at,
    updated_at,
    terminal_at;
