-- Admit a replacement: consume the verified stop proof, advance the provision
-- candidate to the current authorization generation, and set state `starting`
-- (issue 742 P3). The new candidate must exceed the stopped candidate; the
-- caller validates that, the proof, and the absence of unresolved blockers
-- before this update.
UPDATE execution_environments
SET container_generation = $3,
    stop_proof_job_id = NULL,
    stop_proof_generation = NULL,
    state = 'starting',
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
