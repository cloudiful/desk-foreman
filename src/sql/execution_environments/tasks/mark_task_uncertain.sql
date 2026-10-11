-- Mark a task uncertain when a cancel cannot prove nothing was dispatched
-- (issue 753 P1).
--
-- Uncertainty is sticky and terminal-forever for admission purposes: both
-- active indexes keep counting this row, so it still blocks a new task on this
-- environment and binding until an operator resolves it.
UPDATE execution_environment_tasks
SET state = 'uncertain',
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND state <> 'released'
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
