-- Claim the oldest pending Git job assigned to one manager, using DB time.
-- SKIP LOCKED makes concurrent polls race for a single winner; expired rows
-- are never claimable and no claimed/uncertain row is re-claimed.
UPDATE workspace_git_jobs
SET state = 'claimed',
    claimed_at = NOW(),
    updated_at = NOW()
WHERE job_id = (
    SELECT job_id
    FROM workspace_git_jobs
    WHERE assigned_manager_id = $1
      AND state = 'pending'
      AND deadline_at > NOW()
    ORDER BY created_at ASC, job_id ASC
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
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
