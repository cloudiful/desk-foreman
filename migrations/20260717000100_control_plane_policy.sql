ALTER TABLE applications
    ADD COLUMN IF NOT EXISTS default_scopes TEXT[] NOT NULL DEFAULT ARRAY[
        'workspace.read', 'workspace.search', 'workspace.shell', 'workspace.patch'
    ]::TEXT[],
    ADD COLUMN IF NOT EXISTS max_timeout_ms BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_output_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_file_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_sessions BIGINT NULL,
    ADD COLUMN IF NOT EXISTS network_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE mcp_tokens
    ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ NULL,
    ADD COLUMN IF NOT EXISTS scopes TEXT[] NOT NULL DEFAULT ARRAY[
        'workspace.read', 'workspace.search', 'workspace.shell', 'workspace.patch'
    ]::TEXT[],
    ADD COLUMN IF NOT EXISTS max_timeout_ms BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_output_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_file_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_sessions BIGINT NULL,
    ADD COLUMN IF NOT EXISTS network_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE application_tokens
    ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ NULL,
    ADD COLUMN IF NOT EXISTS scopes TEXT[] NOT NULL DEFAULT ARRAY[
        'workspace.read', 'workspace.search', 'workspace.shell', 'workspace.patch'
    ]::TEXT[],
    ADD COLUMN IF NOT EXISTS max_timeout_ms BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_output_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_file_bytes BIGINT NULL,
    ADD COLUMN IF NOT EXISTS max_sessions BIGINT NULL,
    ADD COLUMN IF NOT EXISTS network_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE workspace_bindings
    ADD COLUMN IF NOT EXISTS lifecycle_state TEXT NOT NULL DEFAULT 'active',
    ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ NULL;

ALTER TABLE audit_log
    ADD COLUMN IF NOT EXISTS request_id TEXT NULL,
    ADD COLUMN IF NOT EXISTS session_id BIGINT NULL,
    ADD COLUMN IF NOT EXISTS duration_ms BIGINT NULL,
    ADD COLUMN IF NOT EXISTS status TEXT NULL;

CREATE INDEX IF NOT EXISTS audit_log_created_at_idx ON audit_log (created_at DESC);
CREATE INDEX IF NOT EXISTS audit_log_action_idx ON audit_log (action);
CREATE INDEX IF NOT EXISTS workspace_bindings_lifecycle_idx ON workspace_bindings (lifecycle_state, updated_at);
