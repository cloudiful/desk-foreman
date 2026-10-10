-- The sealed snapshot header for one task (issue 753 P2a).
--
-- Owner scoping is the task id's own task row: the caller has already locked
-- that task inside the same transaction and verified it belongs to the
-- authenticated application and environment.
SELECT
    task_id,
    manifest_sha256,
    file_count,
    total_bytes
FROM execution_environment_task_seeds
WHERE task_id = $1;
