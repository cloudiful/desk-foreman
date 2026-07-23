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
WHERE token_id = $1
LIMIT 1;
