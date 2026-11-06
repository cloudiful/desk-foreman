-- Exact live credential scope for one assigned manager. Foreign, non-claimed,
-- or expired rows return no row.
SELECT
    job_id,
    token,
    operation_id,
    application_id,
    workspace_binding_id,
    assigned_manager_id,
    code_project_id,
    authorization_generation,
    job_kind,
    state,
    canonical_branch,
    expected_base,
    principal_digest,
    lease_owner,
    lease_acquired_at,
    reserved_bytes,
    cleanup_confirmed,
    claimed_at,
    deadline_at,
    terminal_at,
    result,
    error_summary,
    created_at,
    updated_at
FROM workspace_git_jobs
WHERE job_id = $1
  AND assigned_manager_id = $2
  AND state = 'claimed'
  AND deadline_at > NOW();
