-- Owner-scoped stop-proof row for one task release (issue 753 P1).
--
-- Returns exactly the proof facts a release checks, so the caller can verify
-- kind, success, manager, generation, the typed native-task link and
-- positive quiescence inside the same transaction.
SELECT
    job_id,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    state,
    native_task_id,
    result
FROM execution_environment_jobs
WHERE job_id = $1
  AND application_id = $2
  AND environment_id = $3;
