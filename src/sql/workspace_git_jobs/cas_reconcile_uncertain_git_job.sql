UPDATE workspace_git_jobs
SET state = 'succeeded',
    result = $8,
    error_summary = NULL,
    cleanup_confirmed = TRUE,
    terminal_at = NOW(),
    updated_at = NOW()
WHERE job_id = $1
  AND state = 'uncertain'
  AND token = $2
  AND assigned_manager_id = $3
  AND operation_id = $4
  AND application_id = $5
  AND workspace_binding_id = $6
  AND expected_base IS NOT DISTINCT FROM $7
  AND canonical_branch = $9
  AND principal_digest = $10
RETURNING
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
    updated_at;
