-- Read the active lease state for a resource workspace binding scoped to an
-- application. Used by the lease-status read endpoint so callers (e.g. stock)
-- can determine the expected current owner and last refresh time after a 409
-- without parsing human-readable error strings.
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
    write_lease_expires_at,
    NOW() AS db_now
FROM workspace_bindings
WHERE application_id = $1
  AND resource_kind = $2
  AND resource_id = $3
  AND is_active = TRUE
  AND lifecycle_state = 'active'
LIMIT 1;
