INSERT INTO application_tokens (
    application_id,
    token_name,
    token_hash,
    is_active,
    expires_at,
    scopes,
    max_timeout_ms,
    max_output_bytes,
    max_file_bytes,
    max_sessions,
    network_enabled
)
VALUES ($1, $2, $3, TRUE, $4, $5, $6, $7, $8, $9, $10)
RETURNING
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
    network_enabled;
