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
WHERE application_id = $1
  AND external_user_id = $2
  AND workspace_key = $3
  AND is_active = TRUE
  AND lifecycle_state = 'active'
LIMIT 1;
