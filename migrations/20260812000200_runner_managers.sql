CREATE TABLE IF NOT EXISTS runner_managers (
    runner_manager_id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    access_token_hash TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    image TEXT NOT NULL DEFAULT 'desk-foreman-workspace-runner:local',
    network_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    max_output_bytes BIGINT NOT NULL DEFAULT 262144,
    max_timeout_ms BIGINT NOT NULL DEFAULT 600000,
    max_sessions BIGINT NOT NULL DEFAULT 32,
    pids_limit BIGINT NOT NULL DEFAULT 256,
    memory_limit TEXT NOT NULL DEFAULT '1g',
    cpu_limit TEXT NOT NULL DEFAULT '2',
    status TEXT NOT NULL DEFAULT 'pending',
    last_seen_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS runner_managers_name_key
    ON runner_managers (name);

CREATE INDEX IF NOT EXISTS runner_managers_enabled_status_idx
    ON runner_managers (enabled, status, last_seen_at);
