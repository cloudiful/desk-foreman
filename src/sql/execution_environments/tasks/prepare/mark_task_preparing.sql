-- Move one admitted task to `preparing` and link its preparation job
-- (issue 753 P2b).
--
-- Guarded on the task still being `admitted`, so the state change and the job
-- link are single-winner and commit together with the job row.
UPDATE execution_environment_tasks
SET state = 'preparing',
    prepare_job_id = $4,
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND state = 'admitted'
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
    created_at,
    updated_at,
    terminal_at;
