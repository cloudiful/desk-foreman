-- Lock a task-linked execution job after its environment, task and binding.
SELECT
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    admission_generation,
    native_task_id,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at
FROM execution_environment_jobs
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
FOR UPDATE;
