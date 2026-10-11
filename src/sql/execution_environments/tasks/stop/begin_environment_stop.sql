-- First task stop: advance authorization once and persist the potential target.
UPDATE execution_environments
SET generation = generation + 1,
    state = 'stopping',
    container_generation = $5,
    stop_proof_job_id = NULL,
    stop_proof_generation = NULL,
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND generation = $4
  AND container_generation IS NOT DISTINCT FROM $6
  AND state IN ('absent', 'replaced', 'starting', 'ready', 'stopping', 'uncertain')
RETURNING environment_id;
