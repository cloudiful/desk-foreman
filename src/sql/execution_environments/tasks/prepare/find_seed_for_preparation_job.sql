-- The sealed seed of the task behind one preparation job (issue 753 P2b).
--
-- Manager-facing read: the caller has already verified the authenticated
-- manager, the job's kind/state and its assigned manager. This returns only the
-- task, its environment/generation and its binding, so the caller can apply the
-- remaining state checks before any content is fetched.
SELECT
    task.task_id,
    task.environment_id,
    task.application_id,
    task.workspace_binding_id,
    task.state AS task_state,
    task.target_generation AS task_generation,
    task.assigned_manager_id AS task_manager_id,
    env.generation AS environment_generation,
    env.active_native_task_id,
    job.job_kind,
    job.state AS job_state,
    job.assigned_manager_id,
    job.target_generation AS job_generation
FROM execution_environment_jobs AS job
JOIN execution_environment_tasks AS task
    ON task.task_id = job.native_task_id
JOIN execution_environments AS env
    ON env.environment_id = task.environment_id
   AND env.application_id = task.application_id
WHERE job.job_id = $1
  AND job.deadline_at > NOW()
LIMIT 1;
