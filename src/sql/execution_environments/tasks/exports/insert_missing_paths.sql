INSERT INTO execution_environment_task_export_missing_paths (job_id, path)
SELECT $1, path
FROM unnest($2::text[]) AS missing(path);
