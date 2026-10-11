SELECT path, content
FROM execution_environment_task_export_files
WHERE job_id = $1
ORDER BY path;
