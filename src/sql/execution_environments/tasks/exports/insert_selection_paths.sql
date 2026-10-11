INSERT INTO execution_environment_task_export_selection_files (job_id, path)
SELECT $1, path
FROM unnest($2::text[]) AS selected(path);
