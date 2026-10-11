-- Fence one expired native job after environment, task and binding locks.
UPDATE execution_environment_jobs
SET state = 'uncertain',
    result = $5,
    error_summary = $6,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND native_task_id = $4
  AND state IN ('pending', 'claimed')
  AND deadline_at <= clock_timestamp()
RETURNING job_id;
