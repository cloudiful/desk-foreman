-- Insert one durable operation intent for an already-bound environment
-- (issue 742 P3). The caller holds the owner-scoped environment row lock, so
-- `ON CONFLICT (token) DO NOTHING` only reconciles a same-token replay.
INSERT INTO execution_environment_jobs (
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    deadline_at
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT (token) DO NOTHING
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
