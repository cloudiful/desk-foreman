-- Lock the real binding row before any Git admission classification.
-- The caller must keep this transaction open through the advisory lock,
-- capacity check, and insert. The row lock serializes lease changes and
-- binding takeover against the admission.
SELECT
    workspace_binding_id,
    application_id,
    external_user_id,
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
FOR UPDATE;
