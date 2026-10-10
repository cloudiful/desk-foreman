-- Durable environment-operation records (issue 741 P1).
--
-- One row per environment operation intent: ensure or stop for one exact
-- generation of one execution environment. Tenancy (the application owns the
-- environment) is pinned by the composite foreign key below and by the store's
-- own owner-scoped SQL. No dispatch layer consumes these rows in this phase.

-- Additive uniqueness so the composite `(environment_id, application_id)` FK
-- below can reference `execution_environments` without weakening its existing
-- single-column primary key. This is the only change made to the frozen
-- environment identity table.
ALTER TABLE execution_environments
    ADD CONSTRAINT execution_environments_id_application_key
    UNIQUE (environment_id, application_id);

CREATE TABLE IF NOT EXISTS execution_environment_jobs (
    job_id UUID PRIMARY KEY,
    token TEXT NOT NULL UNIQUE,
    environment_id UUID NOT NULL,
    application_id BIGINT NOT NULL,
    assigned_manager_id BIGINT NOT NULL REFERENCES runner_managers (runner_manager_id) ON DELETE RESTRICT,
    job_kind TEXT NOT NULL CHECK (job_kind IN ('ensure', 'stop')),
    target_generation BIGINT NOT NULL CHECK (target_generation >= 0),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'claimed', 'succeeded', 'failed', 'uncertain')),
    claimed_at TIMESTAMPTZ NULL,
    deadline_at TIMESTAMPTZ NOT NULL,
    terminal_at TIMESTAMPTZ NULL,
    result JSONB NULL,
    error_summary TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Owner pair must stay a real environment/application pair; the plain
    -- single-column FKs alone would accept a foreign pairing.
    CONSTRAINT execution_environment_jobs_environment_application_fk
        FOREIGN KEY (environment_id, application_id)
        REFERENCES execution_environments (environment_id, application_id) ON DELETE CASCADE,
    CONSTRAINT execution_environment_jobs_pending_unclaimed CHECK (
        state <> 'pending' OR claimed_at IS NULL
    ),
    CONSTRAINT execution_environment_jobs_claimed_marked CHECK (
        state <> 'claimed' OR claimed_at IS NOT NULL
    ),
    CONSTRAINT execution_environment_jobs_terminal_requires_state CHECK (
        (state IN ('succeeded', 'failed', 'uncertain') AND terminal_at IS NOT NULL)
        OR (state NOT IN ('succeeded', 'failed', 'uncertain') AND terminal_at IS NULL)
    ),
    CONSTRAINT execution_environment_jobs_error_summary_bounded CHECK (
        error_summary IS NULL OR length(error_summary) <= 1024
    )
);

CREATE INDEX IF NOT EXISTS execution_environment_jobs_environment_idx
    ON execution_environment_jobs (environment_id);

CREATE INDEX IF NOT EXISTS execution_environment_jobs_assigned_manager_idx
    ON execution_environment_jobs (assigned_manager_id);

-- At most one active ensure per environment across ALL generations.
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_jobs_active_ensure_key
    ON execution_environment_jobs (environment_id)
    WHERE job_kind = 'ensure'
      AND state IN ('pending', 'claimed', 'uncertain');

-- At most one active stop per environment. Ensure and stop rows are durable
-- records, not runnable concurrent operations.
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_jobs_active_stop_key
    ON execution_environment_jobs (environment_id)
    WHERE job_kind = 'stop'
      AND state IN ('pending', 'claimed', 'uncertain');
