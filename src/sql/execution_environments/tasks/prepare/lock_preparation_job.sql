-- Lock one preparation job after its environment, task and binding have been
-- locked. The clock check is evaluated after those potentially waiting locks.
SELECT
    job_id,
    native_task_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    state,
    deadline_at > clock_timestamp() AS deadline_valid
FROM execution_environment_jobs
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
FOR UPDATE;
