-- Authoritative Git Smart HTTP upstream metadata for a workspace binding
-- (issue 1180 P27).
--
-- One row names one remote alias a workspace may push to. Credentials are
-- stored only as an authenticated ciphertext envelope: username, ciphertext and
-- update time are all present or all absent. The row carries no network state,
-- so `enabled` is metadata only and never a health claim.

CREATE TABLE IF NOT EXISTS workspace_git_upstreams (
    upstream_id BIGSERIAL PRIMARY KEY,
    workspace_binding_id BIGINT NOT NULL
        REFERENCES workspace_bindings (workspace_binding_id) ON DELETE CASCADE,
    remote_alias TEXT NOT NULL,
    target_url TEXT NOT NULL,
    credential_username TEXT NULL,
    credential_ciphertext BYTEA NULL,
    credential_updated_at TIMESTAMPTZ NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    capability_generation BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT workspace_git_upstreams_binding_alias_key
        UNIQUE (workspace_binding_id, remote_alias),
    CONSTRAINT workspace_git_upstreams_alias_format CHECK (
        remote_alias ~ '^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$'
    ),
    CONSTRAINT workspace_git_upstreams_target_url_bounded CHECK (
        char_length(target_url) BETWEEN 1 AND 2048
    ),
    CONSTRAINT workspace_git_upstreams_credential_shape CHECK (
        (
            credential_username IS NULL
            AND credential_ciphertext IS NULL
            AND credential_updated_at IS NULL
        )
        OR (
            credential_username IS NOT NULL
            AND credential_ciphertext IS NOT NULL
            AND credential_updated_at IS NOT NULL
        )
    ),
    CONSTRAINT workspace_git_upstreams_capability_generation_positive CHECK (
        capability_generation >= 1
    )
);

CREATE INDEX IF NOT EXISTS workspace_git_upstreams_binding_id_idx
    ON workspace_git_upstreams (workspace_binding_id);
