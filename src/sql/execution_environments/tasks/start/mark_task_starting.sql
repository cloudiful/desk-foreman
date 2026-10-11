-- Link one native start job while moving a prepared task to `starting`.
UPDATE execution_environment_tasks
SET state = 'starting',
    container_generation = $4,
    start_job_id = $5,
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND state = 'prepared'
  AND target_generation = $4
  AND container_generation IS NULL
  AND prepare_job_id = $6
  AND start_job_id IS NULL
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
