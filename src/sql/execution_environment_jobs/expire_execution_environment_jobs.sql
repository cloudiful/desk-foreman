UPDATE execution_environment_jobs
SET state = 'uncertain',
    terminal_at = NOW(),
    updated_at = NOW()
WHERE application_id = $1
  AND environment_id = $2
  AND state IN ('pending', 'claimed')
  AND deadline_at <= NOW()
RETURNING
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at;
