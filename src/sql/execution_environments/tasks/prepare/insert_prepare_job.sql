-- Insert one durable native-task preparation job (issue 753 P2b).
--
-- The environment/application pair comes from the environment row itself, and
-- the assigned manager must be a real enabled manager, so a caller can never
-- name either. `native_task_id` is set and required for this kind.
--
-- `ON CONFLICT DO NOTHING` covers both the token unique index and the active
-- preparation partial unique index, so a racing prepare returns no row and the
-- caller resolves it by re-reading instead of raising.
INSERT INTO execution_environment_jobs (
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    native_task_id,
    deadline_at
)
SELECT
    $1,
    $2,
    env.environment_id,
    env.application_id,
    $3,
    'prepare_native_task',
    $4,
    $5,
    $6
FROM execution_environments AS env
JOIN runner_managers AS mgr
    ON mgr.runner_manager_id = $3
   AND mgr.enabled = TRUE
WHERE env.environment_id = $7
  AND env.application_id = $8
ON CONFLICT DO NOTHING
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
