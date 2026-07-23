UPDATE mcp_tokens
SET expires_at = COALESCE($2, expires_at),
    scopes = COALESCE($3, scopes),
    max_timeout_ms = COALESCE($4, max_timeout_ms),
    max_output_bytes = COALESCE($5, max_output_bytes),
    max_file_bytes = COALESCE($6, max_file_bytes),
    max_sessions = COALESCE($7, max_sessions),
    network_enabled = COALESCE($8, network_enabled)
WHERE token_id = $1
RETURNING token_id, token_name, created_by AS user_id, is_active, created_at,
          last_used_at, expires_at, scopes, max_timeout_ms, max_output_bytes,
          max_file_bytes, max_sessions, network_enabled;
