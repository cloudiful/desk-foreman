-- The durable preparation job linked to one task (issue 753 P2b).
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
WHERE native_task_id = $1
  AND job_kind = 'prepare_native_task'
LIMIT 1;
