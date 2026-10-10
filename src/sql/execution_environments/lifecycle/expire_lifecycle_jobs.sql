-- Fail closed: overdue pending/claimed jobs of one owned environment become
-- `uncertain` before any lifecycle state decision (issue 742 P3). Expiry never
-- marks a container ready or stopped; it only retains the blocker.
UPDATE execution_environment_jobs
SET state = 'uncertain',
    terminal_at = NOW(),
    updated_at = NOW()
WHERE application_id = $1
  AND environment_id = $2
  AND state IN ('pending', 'claimed')
  AND deadline_at <= NOW()
RETURNING
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at;
