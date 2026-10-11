-- No stop proof exists after failure, false quiescence or expiry.
UPDATE execution_environments
SET state = 'uncertain',
    stop_proof_job_id = NULL,
    stop_proof_generation = NULL,
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND container_generation = $4
  AND state IN ('stopping', 'uncertain')
RETURNING environment_id;
