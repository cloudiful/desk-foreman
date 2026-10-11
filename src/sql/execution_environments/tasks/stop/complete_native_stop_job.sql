-- Complete a claimed task-aware stop, recording success or uncertainty.
UPDATE execution_environment_jobs
SET state = $4,
    result = $5,
    error_summary = $6,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND native_task_id = $7
  AND job_kind = 'stop'
  AND assigned_manager_id = $8
  AND state = 'claimed'
  AND deadline_at > clock_timestamp()
RETURNING job_id;
