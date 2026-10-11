-- Insert one original-manager native task execution job.
INSERT INTO execution_environment_jobs (
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    admission_generation,
    native_task_id,
    deadline_at
)
SELECT
    $1,
    $2,
    env.environment_id,
    env.application_id,
    $5,
    $6,
    $7,
    $11,
    $8,
    $9
FROM execution_environments AS env
JOIN runner_managers AS mgr
    ON mgr.runner_manager_id = $5
   AND mgr.enabled = TRUE
   AND mgr.status = 'online'
   AND mgr.last_seen_at > NOW() - ($10::double precision * INTERVAL '1 second')
JOIN execution_environment_tasks AS task
    ON task.task_id = $8
   AND task.environment_id = env.environment_id
   AND task.application_id = env.application_id
WHERE env.environment_id = $3
  AND env.application_id = $4
ON CONFLICT DO NOTHING
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
