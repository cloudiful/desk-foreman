-- First stop at the expected authorization generation: increment the
-- authorization generation exactly once, preserve the provision candidate,
-- clear any previous proof, and set state `stopping` (issue 742 P3).
UPDATE execution_environments
SET generation = generation + 1,
    state = 'stopping',
    stop_proof_job_id = NULL,
    stop_proof_generation = NULL,
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
