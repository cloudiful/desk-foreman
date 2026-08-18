UPDATE approval_settings
SET enabled = $1,
    endpoint = $2,
    model = $3,
    timeout_ms = $4,
    max_input_bytes = $5,
    max_concurrent = $6,
    api_key_ciphertext = $7,
    api_key_nonce = $8,
    api_key_key_version = $9,
    updated_at = NOW()
WHERE settings_id = 1
RETURNING
    settings_id,
    enabled,
    endpoint,
    model,
    timeout_ms,
    max_input_bytes,
    max_concurrent,
    api_key_ciphertext,
    api_key_nonce,
    api_key_key_version,
    updated_at;
