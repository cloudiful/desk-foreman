-- One durable operation row by opaque replay token within one owned
-- environment (issue 742 P3).
SELECT
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
    updated_at
FROM execution_environment_jobs
WHERE application_id = $1
  AND environment_id = $2
  AND token = $3
LIMIT 1;
