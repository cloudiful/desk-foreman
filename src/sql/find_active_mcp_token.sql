SELECT
    u.user_id,
    u.login_name,
    u.password_hash,
    u.display_name,
    u.email,
    u.timezone,
    u.workspace_root,
    u.is_admin,
    u.is_active,
    u.must_change_password,
    u.deleted_at,
    u.last_login_at,
    u.created_at,
    u.updated_at,
    t.expires_at,
    t.scopes,
    t.max_timeout_ms,
    t.max_output_bytes,
    t.max_file_bytes,
    t.max_sessions,
    t.network_enabled
FROM mcp_tokens t
JOIN users u ON u.user_id = t.created_by
WHERE t.token_hash = $1
  AND t.is_active = TRUE
  AND (t.expires_at IS NULL OR t.expires_at > NOW())
  AND u.deleted_at IS NULL
  AND u.is_active = TRUE
LIMIT 1;
