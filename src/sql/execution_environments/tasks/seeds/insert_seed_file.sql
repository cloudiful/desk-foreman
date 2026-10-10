-- Seal one file of a snapshot (issue 753 P2a).
--
-- Rows are inserted in path order so the stored ordering matches the digest,
-- and `(task_id, path)` makes a duplicate path impossible at the storage
-- layer even if an application-side check ever missed one.
INSERT INTO execution_environment_task_seed_files (task_id, path, content)
VALUES ($1, $2, $3);
