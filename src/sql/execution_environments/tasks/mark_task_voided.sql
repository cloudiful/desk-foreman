-- Void a never-started task: released without any writer ever existing
-- (issue 753 P1).
--
-- The caller proves `container_generation IS NULL` and that no durable job
-- references this task, so there is provably nothing to stop.
UPDATE execution_environment_tasks
SET state = 'released',
    terminal_at = NOW(),
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND container_generation IS NULL
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
