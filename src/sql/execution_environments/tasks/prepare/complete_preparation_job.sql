-- Complete one already-locked native preparation job while its owner and task
-- locks are held. The deadline is checked again at the write boundary.
UPDATE execution_environment_jobs
SET state = $4,
    result = $5,
    error_summary = $6,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND assigned_manager_id = $7
  AND job_kind = 'prepare_native_task'
  AND native_task_id = $8
  AND state = 'claimed'
  AND deadline_at > clock_timestamp()
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
