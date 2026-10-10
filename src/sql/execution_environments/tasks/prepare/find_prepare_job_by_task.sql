-- The active preparation job of one task (issue 753 P2b).
--
-- Used to resolve a racing or replaying prepare: the durable job is the
-- authority, so the caller reports its current status rather than enqueueing a
-- second one.
SELECT
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    native_task_id,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at
FROM execution_environment_jobs
WHERE native_task_id = $1
  AND job_kind = 'prepare_native_task'
ORDER BY created_at ASC, job_id ASC
LIMIT 1;
