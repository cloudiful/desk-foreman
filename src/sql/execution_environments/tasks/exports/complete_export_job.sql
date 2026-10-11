UPDATE execution_environment_jobs
SET state = 'succeeded',
    terminal_at = clock_timestamp(),
    result = $6,
    error_summary = NULL,
    updated_at = clock_timestamp()
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND native_task_id = $4
  AND assigned_manager_id = $5
  AND job_kind = 'export_native_task'
  AND state = 'claimed'
  AND deadline_at > clock_timestamp()
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
