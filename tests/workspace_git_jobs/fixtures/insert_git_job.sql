-- Seed one Git job in an explicit shape. Admission itself always inserts
-- `pending`; this fixture exists only for claimed, uncertain, and expiry cases
-- that no production caller can create yet.
INSERT INTO workspace_git_jobs (
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
    claimed_at,
    deadline_at,
    cleanup_confirmed,
    terminal_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    'sync', $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19
)
RETURNING job_id;
