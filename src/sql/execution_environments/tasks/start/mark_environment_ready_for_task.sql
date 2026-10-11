-- Mark a native candidate ready only after its task start proof succeeds.
UPDATE execution_environments
SET state = 'ready',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND generation = $4
  AND container_generation = $4
  AND state = 'starting'
RETURNING environment_id;
