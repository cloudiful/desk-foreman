-- Retain native task anchors after a failed or expired start.
UPDATE execution_environment_tasks
SET state = 'uncertain',
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND start_job_id = $4
  AND state IN ('starting', 'uncertain')
RETURNING task_id;
