SELECT
    job_id,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    state,
    native_task_id,
    result
FROM execution_environment_jobs
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
FOR UPDATE;
