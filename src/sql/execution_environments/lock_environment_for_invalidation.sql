-- Serialize invalidation with native task admission before mutating generation.
SELECT active_native_task_id IS NOT NULL
FROM execution_environments
WHERE environment_id = $1
  AND application_id = $2
FOR UPDATE;
