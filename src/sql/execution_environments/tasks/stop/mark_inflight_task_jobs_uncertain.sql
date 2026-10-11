-- Fence outstanding preparation/start acknowledgments before issuing task stop.
UPDATE execution_environment_jobs
SET state = 'uncertain',
    result = $2,
    error_summary = $3,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE native_task_id = $1
  AND job_kind IN ('prepare_native_task', 'start_native_task')
  AND state IN ('pending', 'claimed')
RETURNING job_id;
