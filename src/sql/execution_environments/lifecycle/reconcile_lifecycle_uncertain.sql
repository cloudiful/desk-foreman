-- Reconcile an unconfirmed stop: retain the candidate and downgrade a
-- `stopping` environment to `uncertain` so a recovery stop can be admitted
-- (issue 742 P3). Overdue pending/claimed stop rows were already expired to
-- `uncertain` in the same transaction.
UPDATE execution_environments
SET state = 'uncertain',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND state = 'stopping'
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
