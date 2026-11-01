CREATE TABLE runner_owner_stop_fence (
    owner_key TEXT PRIMARY KEY,
    application_id BIGINT NOT NULL CHECK (application_id > 0),
    workspace_binding_id BIGINT NOT NULL CHECK (workspace_binding_id > 0),
    principal_digest TEXT NOT NULL CHECK (principal_digest ~ '^[0-9a-f]{64}$'),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (owner_key = 'application_principal:' || application_id::text || ':' || workspace_binding_id::text || ':' || principal_digest)
);

CREATE TABLE runner_principal_admission (
    owner_key TEXT NOT NULL,
    application_id BIGINT NOT NULL CHECK (application_id > 0),
    workspace_binding_id BIGINT NOT NULL CHECK (workspace_binding_id > 0),
    principal_digest TEXT NOT NULL CHECK (principal_digest ~ '^[0-9a-f]{64}$'),
    runner_manager_id BIGINT NOT NULL CHECK (runner_manager_id > 0),
    container_id TEXT NOT NULL CHECK (octet_length(container_id) BETWEEN 1 AND 256 AND container_id !~ '[[:space:][:cntrl:]]'),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    stopped_at TIMESTAMPTZ,
    CHECK (owner_key = 'application_principal:' || application_id::text || ':' || workspace_binding_id::text || ':' || principal_digest),
    UNIQUE (runner_manager_id, owner_key, container_id)
);

CREATE INDEX runner_principal_admission_binding_active_idx ON runner_principal_admission (workspace_binding_id, owner_key) WHERE stopped_at IS NULL;
CREATE INDEX runner_owner_stop_fence_binding_idx ON runner_owner_stop_fence (workspace_binding_id, owner_key);
