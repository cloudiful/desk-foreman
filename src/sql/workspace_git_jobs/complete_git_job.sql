-- Compare-and-set completion by the assigned manager and token.
-- Success/failed require confirmed cleanup and become terminal; uncertain
-- retains the reservation with no terminal timestamp.
UPDATE workspace_git_jobs
SET state = $4,
    result = $5,
    error_summary = $6,
    cleanup_confirmed = $7,
    terminal_at = CASE WHEN $4 = 'uncertain' THEN NULL ELSE NOW() END,
    updated_at = NOW()
WHERE job_id = $1
  AND token = $2
  AND assigned_manager_id = $3
  AND state = 'claimed'
  AND deadline_at > NOW()
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
