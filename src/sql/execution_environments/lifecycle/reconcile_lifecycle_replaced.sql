-- Record a confirmed stop proof: state `replaced`, proof job UUID and stopped
-- candidate generation (issue 742 P3). Replaces any previous proof.
UPDATE execution_environments
SET state = 'replaced',
    stop_proof_job_id = $3,
    stop_proof_generation = $4,
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
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
