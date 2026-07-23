CREATE TABLE IF NOT EXISTS applications (
    application_id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    workspace_template TEXT NULL,
    default_shell TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS application_tokens (
    token_id BIGSERIAL PRIMARY KEY,
    application_id BIGINT NOT NULL REFERENCES applications(application_id) ON DELETE CASCADE,
    token_name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    last_used_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS application_tokens_application_id_idx
    ON application_tokens (application_id);

CREATE TABLE IF NOT EXISTS workspace_bindings (
    workspace_binding_id BIGSERIAL PRIMARY KEY,
    application_id BIGINT NOT NULL REFERENCES applications(application_id) ON DELETE CASCADE,
    external_user_id TEXT NOT NULL,
    workspace_key TEXT NOT NULL DEFAULT 'default',
    external_user_hash TEXT NOT NULL,
    workspace_root TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT workspace_bindings_identity_key UNIQUE (application_id, external_user_id, workspace_key)
);

CREATE INDEX IF NOT EXISTS workspace_bindings_application_id_idx
    ON workspace_bindings (application_id);

CREATE INDEX IF NOT EXISTS workspace_bindings_lookup_idx
    ON workspace_bindings (application_id, external_user_id, workspace_key);

ALTER TABLE audit_log
    ADD COLUMN IF NOT EXISTS actor_application_id BIGINT NULL REFERENCES applications(application_id),
    ADD COLUMN IF NOT EXISTS workspace_binding_id BIGINT NULL REFERENCES workspace_bindings(workspace_binding_id),
    ADD COLUMN IF NOT EXISTS external_user_id TEXT NULL;
