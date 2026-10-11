INSERT INTO execution_environment_task_export_files (job_id, path, content)
SELECT $1, path, content
FROM unnest($2::text[], $3::text[]) AS output(path, content);
