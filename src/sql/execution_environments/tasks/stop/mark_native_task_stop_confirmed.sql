-- Keep a positively stopped task in `stopping` until explicit release.
UPDATE execution_environment_tasks
SET updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND stop_job_id = $4
  AND state = 'stopping'
  AND container_generation = $5
RETURNING task_id;
