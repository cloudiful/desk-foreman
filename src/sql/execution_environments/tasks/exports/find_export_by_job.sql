SELECT
    job_id,
    task_id,
    environment_id,
    application_id,
    workspace_binding_id,
    assigned_manager_id,
    target_generation,
    stop_job_id,
    selection_sha256,
    selection_file_count,
    output_manifest_sha256,
    output_file_count,
    output_missing_count,
    output_total_bytes,
    completed_at
FROM execution_environment_task_exports
WHERE job_id = $1
  AND environment_id = $2
  AND application_id = $3;
