-- Keep a possibly-created native container as an uncertain candidate.
UPDATE execution_environments
SET state = 'uncertain',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND container_generation = $4
  AND state IN ('starting', 'ready', 'stopping', 'uncertain')
RETURNING environment_id;
