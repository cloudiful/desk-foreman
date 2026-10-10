-- Authenticated asynchronous execution environment lifecycle and stop proof
-- (issue 742 P3).
--
-- Additive forward migration only: extends `execution_environments` with an
-- immutable manager binding, a provision container candidate and a consumed
-- stop proof, and extends `execution_environment_jobs` with an explicit
-- `superseded` terminal state. No prior migration is edited.

-- 1. Environment lifecycle metadata.
ALTER TABLE execution_environments
    ADD COLUMN assigned_manager_id BIGINT NULL
        REFERENCES runner_managers (runner_manager_id) ON DELETE RESTRICT,
    ADD COLUMN container_generation BIGINT NULL
        CHECK (container_generation IS NULL OR container_generation >= 0),
    ADD COLUMN stop_proof_job_id UUID NULL,
    ADD COLUMN stop_proof_generation BIGINT NULL
        CHECK (stop_proof_generation IS NULL OR stop_proof_generation >= 0);

-- 2. Environment state allowlist gains `uncertain`.
ALTER TABLE execution_environments
    DROP CONSTRAINT execution_environments_state_check;
ALTER TABLE execution_environments
    ADD CONSTRAINT execution_environments_state_check CHECK (
        state IN (
            'absent', 'starting', 'ready', 'stopping', 'replaced',
            'unreachable', 'uncertain'
        )
    );

-- 3. Job state allowlist gains `superseded` (a terminal stop-proof
--    cancellation; never an ensure success).
ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_state_check;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_state_check CHECK (
        state IN (
            'pending', 'claimed', 'succeeded', 'failed', 'uncertain',
            'superseded'
        )
    );

ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_terminal_requires_state;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_terminal_requires_state CHECK (
        (
            state IN ('succeeded', 'failed', 'uncertain', 'superseded')
            AND terminal_at IS NOT NULL
        )
        OR (
            state NOT IN ('succeeded', 'failed', 'uncertain', 'superseded')
            AND terminal_at IS NULL
        )
    );

-- 4. Stop active blocker becomes pending/claimed only: a prior `uncertain`
--    stop stays recorded as evidence but no longer blocks a recovery stop.
--    The ensure active blocker keeps pending/claimed/uncertain across all
--    generations.
DROP INDEX IF EXISTS execution_environment_jobs_active_stop_key;
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_jobs_active_stop_key
    ON execution_environment_jobs (environment_id)
    WHERE job_kind = 'stop'
      AND state IN ('pending', 'claimed');

CREATE INDEX IF NOT EXISTS execution_environments_assigned_manager_idx
    ON execution_environments (assigned_manager_id);
