UPDATE workspace_bindings
SET
    write_lease_owner = $1,
    write_lease_acquired_at = NOW(),
    write_lease_expires_at = NOW() + ($2::BIGINT * INTERVAL '1 second'),
    updated_at = NOW()
WHERE workspace_binding_id = $3
  AND (
        write_lease_owner IS NULL
        OR write_lease_owner = $1
        OR write_lease_expires_at < NOW()
      )
RETURNING
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
    write_lease_expires_at;
