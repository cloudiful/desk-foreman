-- Claim the oldest pending durable environment job assigned to one manager
-- (issue 742 P2). `FOR UPDATE SKIP LOCKED` makes concurrent polls race for a
-- single winner; expired and non-pending rows are never claimable.
UPDATE execution_environment_jobs
SET state = 'claimed',
    claimed_at = NOW(),
    updated_at = NOW()
WHERE job_id = (
    SELECT job_id
    FROM execution_environment_jobs
    WHERE assigned_manager_id = $1
      AND state = 'pending'
      AND deadline_at > NOW()
    ORDER BY created_at ASC, job_id ASC
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
RETURNING
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    admission_generation,
    native_task_id,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at;
