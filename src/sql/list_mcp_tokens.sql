SELECT
    token_id,
    token_name,
    created_by AS user_id,
    is_active,
    created_at,
    last_used_at,
    expires_at,
    scopes,
    max_timeout_ms,
    max_output_bytes,
    max_file_bytes,
    max_sessions,
    network_enabled
FROM mcp_tokens
WHERE ($1::TEXT IS NULL OR token_name ILIKE '%' || $1 || '%')
  AND ($2::BIGINT IS NULL OR created_by = $2)
  AND ($3::BOOLEAN IS NULL OR is_active = $3)
ORDER BY created_at DESC, token_id DESC
LIMIT $4 OFFSET $5;
