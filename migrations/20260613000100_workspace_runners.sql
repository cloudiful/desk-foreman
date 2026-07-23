CREATE TABLE IF NOT EXISTS workspace_runners (
    runner_id BIGSERIAL PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_user_id BIGINT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    owner_workspace_binding_id BIGINT NULL REFERENCES workspace_bindings(workspace_binding_id) ON DELETE CASCADE,
    container_name TEXT NOT NULL UNIQUE,
    container_id TEXT NULL,
    runtime TEXT NOT NULL DEFAULT 'docker',
    runtime_class TEXT NULL,
    image_name TEXT NOT NULL,
    status TEXT NOT NULL,
    network_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    workspace_root TEXT NOT NULL,
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_error TEXT NULL,
    CONSTRAINT workspace_runners_owner_check CHECK (
        (owner_kind = 'user' AND owner_user_id IS NOT NULL AND owner_workspace_binding_id IS NULL)
        OR
        (owner_kind = 'workspace_binding' AND owner_user_id IS NULL AND owner_workspace_binding_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS workspace_runners_user_owner_idx
    ON workspace_runners (owner_user_id)
    WHERE owner_kind = 'user';

CREATE UNIQUE INDEX IF NOT EXISTS workspace_runners_binding_owner_idx
    ON workspace_runners (owner_workspace_binding_id)
    WHERE owner_kind = 'workspace_binding';

CREATE INDEX IF NOT EXISTS workspace_runners_status_last_active_idx
    ON workspace_runners (status, last_active_at);
