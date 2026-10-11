UPDATE execution_environment_jobs
SET state = $6,
    result = $7,
    error_summary = $8,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND token = $4
  AND assigned_manager_id = $5
  AND native_task_id IS NULL
  AND state = 'claimed'
  AND deadline_at > NOW()
RETURNING
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
    updated_at;
