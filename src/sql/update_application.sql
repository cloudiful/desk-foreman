UPDATE applications
SET
    name = $2,
    is_active = $3,
    workspace_template = $4,
    default_shell = $5,
    default_scopes = COALESCE($6, default_scopes),
    max_timeout_ms = $7,
    max_output_bytes = $8,
    max_file_bytes = $9,
    max_sessions = $10,
    network_enabled = $11,
    updated_at = NOW()
WHERE application_id = $1
RETURNING
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
    network_enabled;
