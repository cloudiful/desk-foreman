-- Complete one claimed native start before its deadline.
UPDATE execution_environment_jobs
SET state = $7,
    result = $4,
    error_summary = $8,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND native_task_id = $5
  AND job_kind = 'start_native_task'
  AND assigned_manager_id = $6
  AND state = 'claimed'
  AND deadline_at > clock_timestamp()
RETURNING job_id;
