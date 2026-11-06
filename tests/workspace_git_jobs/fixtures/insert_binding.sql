INSERT INTO workspace_bindings (
    application_id,
    external_user_id,
    workspace_key,
    external_user_hash,
    workspace_root,
    resource_kind,
    resource_id,
    write_lease_owner,
    write_lease_acquired_at,
    write_lease_expires_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
RETURNING workspace_binding_id;
