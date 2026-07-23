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
WHERE ($1::BIGINT IS NULL OR application_id = $1)
  AND ($2::TEXT IS NULL OR external_user_id = $2)
  AND ($3::TEXT IS NULL OR workspace_key = $3)
  AND ($4::BOOLEAN IS NULL OR is_active = $4)
ORDER BY last_used_at DESC, workspace_binding_id DESC
LIMIT $5 OFFSET $6;
