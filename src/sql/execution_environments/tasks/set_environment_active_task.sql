-- Point the environment at its active native task (issue 753 P1).
--
-- Guarded on the anchor still being unset so the write is a single-winner
-- operation even if a future caller retries without holding the row lock.
UPDATE execution_environments
SET active_native_task_id = $1,
    updated_at = NOW()
WHERE environment_id = $2
  AND application_id = $3
  AND active_native_task_id IS NULL;
