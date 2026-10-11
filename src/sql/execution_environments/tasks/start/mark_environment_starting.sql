-- Persist a native start candidate without claiming that it is already running.
UPDATE execution_environments
SET state = 'starting',
    container_generation = $4,
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND generation = $4
  AND container_generation IS NOT DISTINCT FROM $5
  AND state IN ('absent', 'replaced')
RETURNING environment_id;
