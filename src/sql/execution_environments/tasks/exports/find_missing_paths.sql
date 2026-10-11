SELECT path
FROM execution_environment_task_export_missing_paths
WHERE job_id = $1
ORDER BY path;
