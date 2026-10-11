-- Trusted manager-only lookup of one durable environment job by its stable
-- UUID (issue 742 P2). Not a client store: only the authenticated
-- runner-manager result path calls this, and it re-verifies the assigned
-- manager against the row before any evaluation.
SELECT
    job_id,
    token,
    environment_id,
    application_id,
    assigned_manager_id,
    job_kind,
    target_generation,
    admission_generation,
    native_task_id,
    state,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at
FROM execution_environment_jobs
WHERE job_id = $1
LIMIT 1;
