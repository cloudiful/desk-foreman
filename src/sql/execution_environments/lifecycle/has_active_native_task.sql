-- Read the active task anchor after its caller holds the environment row lock.
SELECT active_native_task_id IS NOT NULL
FROM execution_environments
WHERE environment_id = $1
  AND application_id = $2;
