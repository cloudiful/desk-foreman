-- Owner-scoped environment row lock for one task admission/cancel/release
-- (issue 753 P1). The caller holds this lock through the whole transaction, so
-- the active-task anchor and the task row cannot move underneath it.
SELECT
    environment_id,
    application_id,
    external_user_id,
    generation,
    state,
    assigned_manager_id,
    container_generation,
    stop_proof_job_id,
    stop_proof_generation,
    active_native_task_id
FROM execution_environments
WHERE environment_id = $1
  AND application_id = $2
FOR UPDATE;
