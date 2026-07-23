INSERT INTO workspace_bindings (
    application_id,
    external_user_id,
    workspace_key,
    external_user_hash,
    workspace_root,
    is_active,
    lifecycle_state
)
VALUES ($1, $2, $3, $4, $5, TRUE, 'active')
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
    archived_at;
