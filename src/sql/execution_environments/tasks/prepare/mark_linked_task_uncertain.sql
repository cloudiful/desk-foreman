-- Mark the task linked to a preparation job uncertain (issue 753 P2b).
--
-- Used when a preparation cannot be confirmed or its job expires: the task
-- keeps its exclusive environment and workspace anchors, is never released,
-- and is never re-dispatched automatically.
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
