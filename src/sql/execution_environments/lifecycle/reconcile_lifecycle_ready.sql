-- Reconcile a matching successful ensure: mark the environment `ready` only
-- from `starting` (issue 742 P3). The caller has already proved the
-- authorization generation equals the candidate and that no stop is
-- outstanding.
UPDATE execution_environments
SET state = 'ready',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND state = 'starting'
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
