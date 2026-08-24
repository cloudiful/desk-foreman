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
    network_enabled
FROM applications
WHERE ($1::TEXT IS NULL OR name ILIKE '%' || $1 || '%')
  AND ($2::BOOLEAN IS NULL OR is_active = $2)
ORDER BY created_at DESC, application_id DESC
LIMIT $3 OFFSET $4;
