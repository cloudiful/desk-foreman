SELECT
    application_id,
    name,
    is_active,
    workspace_template,
    default_shell,
    created_at,
    updated_at,
    default_scopes,
    max_timeout_ms,
    max_output_bytes,
    max_file_bytes,
    max_sessions,
    network_enabled,
    approval_mode,
    approval_endpoint,
    approval_model,
    approval_timeout_ms,
    approval_max_input_bytes,
    approval_max_concurrent,
    approval_max_output_tokens,
    approval_api_key_ciphertext IS NOT NULL AS approval_api_key_configured
FROM applications
WHERE application_id = $1
LIMIT 1;
