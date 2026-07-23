SELECT
    ws.session_id,
    ws.user_id,
    ws.expires_at,
    ws.created_at,
    ws.last_seen_at,
    ws.revoked_at,
    u.user_id,
    u.login_name,
    u.password_hash,
    u.display_name,
    u.email,
    u.timezone,
    u.workspace_root,
    u.is_admin,
    u.is_active,
    u.deleted_at,
    u.last_login_at,
    u.created_at,
    u.updated_at
FROM web_sessions ws
JOIN users u ON u.user_id = ws.user_id
WHERE ws.session_id = $1
  AND ws.revoked_at IS NULL
  AND ws.expires_at > NOW()
LIMIT 1;
