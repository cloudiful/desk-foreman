-- Clear the environment's active task pointer on release/void (issue 753 P1).
--
-- Guarded on the exact task id: a stale cleanup must never detach a newer
-- task that already owns the environment.
UPDATE execution_environments
SET active_native_task_id = NULL,
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3;
