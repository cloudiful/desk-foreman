UPDATE approval_settings
SET endpoint = $1,
    model = $2,
    timeout_ms = $3,
    max_input_bytes = $4,
    max_concurrent = $5,
    updated_at = NOW()
WHERE settings_id = 1
RETURNING
    settings_id,
    endpoint,
    model,
    timeout_ms,
    max_input_bytes,
    max_concurrent,
    updated_at;
