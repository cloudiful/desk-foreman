UPDATE execution_environment_task_exports
SET output_manifest_sha256 = $2,
    output_file_count = $3,
    output_missing_count = $4,
    output_total_bytes = $5,
    completed_at = clock_timestamp()
WHERE job_id = $1
  AND output_manifest_sha256 IS NULL
RETURNING
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
    completed_at;
