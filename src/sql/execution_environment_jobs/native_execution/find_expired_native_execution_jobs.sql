-- Read overdue task-linked rows only after the environment lock is held.
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
WHERE application_id = $1
  AND environment_id = $2
  AND native_task_id IS NOT NULL
  AND state IN ('pending', 'claimed')
  AND deadline_at <= clock_timestamp()
ORDER BY created_at, job_id;
