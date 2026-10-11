-- Mark a task running only after the exact positive native start acknowledgment.
UPDATE execution_environment_tasks
SET state = 'running',
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND state = 'starting'
  AND start_job_id = $4
  AND target_generation = $5
  AND container_generation = $5
RETURNING task_id;
