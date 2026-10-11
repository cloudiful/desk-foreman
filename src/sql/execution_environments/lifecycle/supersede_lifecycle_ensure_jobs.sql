-- Supersede obsolete ensure records matching a confirmed stop proof (issue 742
-- P3): only matching pending/claimed/uncertain ensure rows of the stopped
-- candidate generation and bound manager, recorded as terminal `superseded`
-- with an explicit stop-proof result that is never an ensure success. Stop
-- history is untouched.
UPDATE execution_environment_jobs
SET state = 'superseded',
    terminal_at = NOW(),
    result = $5,
    updated_at = NOW()
WHERE application_id = $1
  AND environment_id = $2
  AND job_kind = 'ensure'
  AND assigned_manager_id = $3
  AND target_generation = $4
  AND state IN ('pending', 'claimed', 'uncertain')
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
