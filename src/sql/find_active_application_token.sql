SELECT
    t.token_id,
    a.application_id,
    a.name,
    a.is_active,
    a.workspace_template,
    a.default_shell,
    a.created_at,
    a.updated_at,
    a.default_scopes,
    a.max_timeout_ms AS app_max_timeout_ms,
    a.max_output_bytes AS app_max_output_bytes,
    a.max_file_bytes AS app_max_file_bytes,
    a.max_sessions AS app_max_sessions,
    a.network_enabled AS app_network_enabled,
    a.approval_mode AS app_approval_mode,
    a.approval_endpoint AS app_approval_endpoint,
    a.approval_model AS app_approval_model,
    a.approval_timeout_ms AS app_approval_timeout_ms,
    a.approval_max_input_bytes AS app_approval_max_input_bytes,
    a.approval_max_concurrent AS app_approval_max_concurrent,
    a.approval_api_key_ciphertext IS NOT NULL AS app_approval_api_key_configured,
    t.expires_at,
    t.scopes,
    t.max_timeout_ms,
    t.max_output_bytes,
    t.max_file_bytes,
    t.max_sessions,
    t.network_enabled
FROM application_tokens t
JOIN applications a ON a.application_id = t.application_id
WHERE t.token_hash = $1
  AND t.is_active = TRUE
  AND (t.expires_at IS NULL OR t.expires_at > NOW())
  AND a.is_active = TRUE
LIMIT 1;
