-- Insert one pending sync reservation. The partial unique index guarantees
-- one active sync per binding; ON CONFLICT makes that a typed no-row result
-- instead of a raw unique violation.
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
    deadline_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    'sync', 'pending', $9, $10, $11, $12, $13, $14, $15
)
ON CONFLICT (workspace_binding_id, job_kind)
    WHERE state IN ('pending', 'claimed', 'uncertain')
    DO NOTHING
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
