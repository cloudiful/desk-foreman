-- Store the native stop proof without releasing its workspace owner.
UPDATE execution_environments
SET state = 'replaced',
    stop_proof_job_id = $4,
    stop_proof_generation = $5,
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND active_native_task_id = $3
  AND container_generation = $5
  AND generation > $5
  AND state = 'stopping'
RETURNING environment_id;
