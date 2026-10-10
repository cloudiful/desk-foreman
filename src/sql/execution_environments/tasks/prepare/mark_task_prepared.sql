-- Mark a task `prepared` once its original manager proved materialization
-- (issue 753 P2b).
--
-- Guarded on the task still being `preparing` and still linked to this exact
-- preparation job, so a late ack after a cancel can never re-mark a task
-- terminal or prepared.
UPDATE execution_environment_tasks
SET state = 'prepared',
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND state = 'preparing'
  AND prepare_job_id = $4
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
