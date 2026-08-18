SELECT
    settings_id,
    enabled,
    endpoint,
    model,
    timeout_ms,
    max_input_bytes,
    max_concurrent,
    max_output_tokens,
    api_key_ciphertext,
    api_key_nonce,
    api_key_key_version,
    updated_at
FROM approval_settings
WHERE settings_id = 1;
