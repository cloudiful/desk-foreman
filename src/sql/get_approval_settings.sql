SELECT
    settings_id,
    endpoint,
    model,
    timeout_ms,
    max_input_bytes,
    max_concurrent,
    updated_at
FROM approval_settings
WHERE settings_id = 1;
