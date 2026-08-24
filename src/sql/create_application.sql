INSERT INTO applications (
    name,
    workspace_template,
    default_shell,
    default_scopes,
    max_timeout_ms,
    max_output_bytes,
    max_file_bytes,
    max_sessions,
    network_enabled
)
VALUES ($1, $2, $3, COALESCE($4, ARRAY['workspace.read', 'workspace.search', 'workspace.shell', 'workspace.patch']::TEXT[]), $5, $6, $7, $8, $9)
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
