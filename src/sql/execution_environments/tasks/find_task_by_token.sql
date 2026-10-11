-- Owner-scoped replay lookup by opaque operation token (issue 753 P1).
--
-- Scoped to the owning application so a foreign token is simply absent: a
-- caller can never learn that another application's task exists.
SELECT
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
    terminal_at
FROM execution_environment_tasks
WHERE operation_token = $1
  AND application_id = $2;
