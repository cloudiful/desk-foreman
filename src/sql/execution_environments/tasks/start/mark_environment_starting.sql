-- Persist a native start candidate without claiming that it is already running.
--
-- `$6` is the task's already-pinned original manager. A cold environment that
-- has never been assigned a manager takes it here, in the same statement and
-- the same transaction as the `starting` candidate. An environment that
-- already names a manager is never retargeted: the guard only accepts the task's
-- own manager, or a NULL manager on the exact absent shape with no candidate.
UPDATE execution_environments
SET state = 'starting',
    assigned_manager_id = COALESCE(assigned_manager_id, $6),
    container_generation = $4,
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND generation = $4
  AND container_generation IS NOT DISTINCT FROM $5
  AND state IN ('absent', 'replaced')
  AND (assigned_manager_id = $6
       OR (assigned_manager_id IS NULL AND state = 'absent' AND container_generation IS NULL))
RETURNING environment_id;
