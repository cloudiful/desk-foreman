-- Retain task ownership after a failed task stop.
UPDATE execution_environment_tasks
SET state = 'uncertain',
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND stop_job_id = $4
  AND state IN ('stopping', 'uncertain')
RETURNING task_id;
