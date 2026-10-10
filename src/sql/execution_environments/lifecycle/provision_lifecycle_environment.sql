-- Admit one first provision: bind the manager if unset, record the provision
-- candidate generation, and set state `starting` (issue 742 P3). Only runs when
-- no candidate exists yet; an existing candidate refuses elsewhere.
UPDATE execution_environments
SET assigned_manager_id = COALESCE(assigned_manager_id, $3),
    container_generation = $4,
    state = 'starting',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND container_generation IS NULL
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
