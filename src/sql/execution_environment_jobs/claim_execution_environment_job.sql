UPDATE execution_environment_jobs
SET state = 'claimed',
    claimed_at = NOW(),
    updated_at = NOW()
WHERE token = $1
  AND application_id = $2
  AND environment_id = $3
  AND assigned_manager_id = $4
  AND state = 'pending'
  AND deadline_at > NOW()
RETURNING
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    native_task_id,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at;
