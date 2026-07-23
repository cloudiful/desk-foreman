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
    archived_at
FROM workspace_bindings
WHERE lifecycle_state = 'archived'
  AND archived_at < $1
ORDER BY archived_at ASC;
