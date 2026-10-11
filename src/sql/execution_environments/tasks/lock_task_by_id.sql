-- Task row lock for one cancel/release mutation (issue 753 P1).
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
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
FOR UPDATE;
