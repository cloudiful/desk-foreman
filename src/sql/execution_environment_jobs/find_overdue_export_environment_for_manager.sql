-- Select the environment of one overdue export job of the authenticated
-- runner manager. The bounded poll preflight expires at most
-- one overdue export job per poll; the deadline and job order make the
-- choice deterministic. Only pending or claimed export jobs of this manager
-- are considered.
SELECT application_id, environment_id
FROM execution_environment_jobs
WHERE assigned_manager_id = $1
  AND job_kind = 'export_native_task'
  AND state IN ('pending', 'claimed')
  AND deadline_at <= clock_timestamp()
ORDER BY deadline_at ASC, job_id ASC
LIMIT 1;
