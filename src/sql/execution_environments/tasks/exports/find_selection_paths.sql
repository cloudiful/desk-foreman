SELECT path
FROM execution_environment_task_export_selection_files
WHERE job_id = $1
ORDER BY path;
