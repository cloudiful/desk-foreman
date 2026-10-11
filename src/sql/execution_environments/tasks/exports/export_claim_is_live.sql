SELECT COALESCE(state = 'claimed' AND deadline_at > clock_timestamp(), FALSE) AS claim_live
FROM execution_environment_jobs
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND native_task_id = $4
  AND assigned_manager_id = $5
  AND job_kind = 'export_native_task';
