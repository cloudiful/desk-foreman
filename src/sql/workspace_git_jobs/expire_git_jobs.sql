-- Expire overdue Git jobs using the database clock.
-- An unclaimed pending job is proven never to have started and becomes
-- failed with confirmed cleanup. A claimed job may have started and becomes
-- uncertain, retaining its reservation and no terminal timestamp.
UPDATE workspace_git_jobs
SET state = CASE WHEN state = 'pending' THEN 'failed' ELSE 'uncertain' END,
    cleanup_confirmed = CASE WHEN state = 'pending' THEN TRUE ELSE FALSE END,
    terminal_at = CASE WHEN state = 'pending' THEN NOW() ELSE NULL END,
    updated_at = NOW()
WHERE state IN ('pending', 'claimed')
  AND deadline_at <= NOW()
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
