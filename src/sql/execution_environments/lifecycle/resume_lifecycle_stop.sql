-- Recovery stop while already stopping/uncertain with the same candidate: set
-- state `stopping` without another generation increment and without any
-- candidate change or manager reassignment (issue 742 P3).
UPDATE execution_environments
SET state = 'stopping',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND generation = $3
RETURNING
    environment_id,
    application_id,
    external_user_id,
    runtime_kind,
    generation,
    state,
    state_volume_ref,
    assigned_manager_id,
    container_generation,
    stop_proof_job_id,
    stop_proof_generation,
    last_active_at,
    created_at,
    updated_at;
