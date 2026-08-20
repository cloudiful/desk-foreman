-- Lock the active workspace binding row for the takeover transaction.
--
-- Returns the pre-update lease state plus NOW() so the caller can
-- classify the request without any application-clock dependency. The
-- row lock (FOR UPDATE) serializes concurrent acquire/renew/takeover
-- against this binding so the eligibility decision and the subsequent
-- UPDATE cannot race against a writer that refreshes the lease between
-- decision and assignment.
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
WHERE workspace_binding_id = $1
  AND is_active = TRUE
  AND lifecycle_state = 'active'
FOR UPDATE;