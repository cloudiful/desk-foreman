UPDATE workspace_bindings
SET
    lifecycle_state = $2,
    is_active = ($2 = 'active'),
    archived_at = CASE WHEN $2 = 'archived' THEN NOW() ELSE NULL END,
    updated_at = NOW()
WHERE workspace_binding_id = $1
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
