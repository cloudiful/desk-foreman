-- Terminal release of one task (issue 753 P1).
--
-- The caller has already proven the matching confirmed stop inside the same
-- transaction; this only records the outcome. Guarded on the task still being
-- non-terminal so a repeated release can never rewrite `terminal_at`.
UPDATE execution_environment_tasks
SET state = 'released',
    terminal_at = NOW(),
    updated_at = NOW()
WHERE task_id = $1
  AND application_id = $2
  AND environment_id = $3
  AND state <> 'released'
RETURNING
    task_id,
    operation_token,
    environment_id,
    application_id,
    workspace_binding_id,
    subject_external_user_id,
    target_generation,
    assigned_manager_id,
    state,
    container_generation,
    prepare_job_id,
    created_at,
    updated_at,
    terminal_at;
