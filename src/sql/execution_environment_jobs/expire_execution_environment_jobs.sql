UPDATE execution_environment_jobs
SET state = 'uncertain',
    terminal_at = NOW(),
    updated_at = NOW()
WHERE application_id = $1
  AND environment_id = $2
  AND (native_task_id IS NULL OR job_kind = 'export_native_task')
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
