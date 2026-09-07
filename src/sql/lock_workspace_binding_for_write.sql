-- Lock the workspace binding row for fenced write admission.
--
-- The caller must hold the returned transaction open from the lease check
-- through the write dispatch so a concurrent lease takeover (which locks
-- the same row via lock_workspace_write_lease_for_takeover.sql) cannot
-- commit between the check and the write. A plain SELECT reread without
-- this row lock leaves exactly that interleaving open for already-admitted
-- old-owner writes (including direct workspace-SDK file operations).
SELECT
    workspace_binding_id,
    application_id,
    external_user_id,
    workspace_key,
    external_user_hash,
    workspace_root,
    is_active,
    last_used_at,
    created_at,
    updated_at,
    lifecycle_state,
    archived_at,
    resource_kind,
    resource_id,
    write_lease_owner,
    write_lease_acquired_at,
    write_lease_expires_at
FROM workspace_bindings
WHERE workspace_binding_id = $1
FOR UPDATE;
