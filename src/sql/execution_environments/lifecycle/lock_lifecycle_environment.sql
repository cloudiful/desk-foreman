-- Owner-scoped row lock for one lifecycle mutation/status read (issue 742 P3).
SELECT
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
    updated_at
FROM execution_environments
WHERE environment_id = $1
  AND application_id = $2
FOR UPDATE;
