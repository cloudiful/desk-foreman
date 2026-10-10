INSERT INTO execution_environment_jobs (
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    deadline_at
)
SELECT
    $1,
    $2,
    env.environment_id,
    env.application_id,
    $3,
    $4,
    $5,
    $6
FROM execution_environments AS env
JOIN runner_managers AS mgr
    ON mgr.runner_manager_id = $3
   AND mgr.enabled = TRUE
WHERE env.environment_id = $7
  AND env.application_id = $8
ON CONFLICT (token) DO NOTHING
RETURNING
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
    updated_at;
