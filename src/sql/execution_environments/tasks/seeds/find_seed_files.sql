-- The sealed files of a snapshot, in canonical path order (issue 753 P2a).
SELECT path, content
FROM execution_environment_task_seed_files
WHERE task_id = $1
ORDER BY path;
