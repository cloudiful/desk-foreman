SELECT
    token_id,
    application_id,
    token_name,
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
FROM application_tokens
WHERE ($1::BIGINT IS NULL OR application_id = $1)
  AND ($2::BOOLEAN IS NULL OR is_active = $2)
ORDER BY created_at DESC, token_id DESC
LIMIT $3 OFFSET $4;
