-- Durable user execution environment identity and generation fencing (issue 737 P1).
--
-- Additive infrastructure only: one row per (application_id, external_user_id)
-- with a stable UUID identity, an opaque agent-neutral runtime kind, a
-- monotonic generation counter and a checked lifecycle state. No container
-- lifecycle, service proxy, mounts, routing or production migration.

CREATE TABLE IF NOT EXISTS execution_environments (
    environment_id UUID PRIMARY KEY,
    application_id BIGINT NOT NULL REFERENCES applications(application_id) ON DELETE CASCADE,
    external_user_id TEXT NOT NULL,
    runtime_kind TEXT NOT NULL,
    generation BIGINT NOT NULL DEFAULT 0 CHECK (generation >= 0),
    state TEXT NOT NULL DEFAULT 'starting' CHECK (state IN ('absent', 'starting', 'ready', 'stopping', 'replaced', 'unreachable')),
    state_volume_ref TEXT NOT NULL CHECK (state_volume_ref <> ''),
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT execution_environments_identity_key UNIQUE (application_id, external_user_id)
);

CREATE INDEX IF NOT EXISTS execution_environments_application_id_idx
    ON execution_environments (application_id);
