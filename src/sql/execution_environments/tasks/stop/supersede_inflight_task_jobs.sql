-- Positive native stop quiescence closes all older task execution blockers.
UPDATE execution_environment_jobs
SET state = 'superseded',
    result = $2,
    error_summary = NULL,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE native_task_id = $1
  AND job_id <> $3
  AND (
      (job_kind IN ('prepare_native_task', 'start_native_task')
       AND state IN ('pending', 'claimed', 'uncertain'))
      OR (job_kind = 'stop' AND state = 'uncertain')
  )
RETURNING job_id;
