-- Read the current workspace binding state to disambiguate 404 (binding
-- does not exist) from 409 (binding exists but is archived/resetting).
-- Used as the pre-transaction probe; the authoritative state is locked
-- via SELECT ... FOR UPDATE once the transaction is opened.
SELECT
    workspace_binding_id,
    application_id,
    workspace_key,
    is_active,
    lifecycle_state,
    resource_kind,
    resource_id,
    write_lease_owner,
    write_lease_acquired_at,
    write_lease_expires_at
FROM workspace_bindings
WHERE workspace_binding_id = $1
LIMIT 1;