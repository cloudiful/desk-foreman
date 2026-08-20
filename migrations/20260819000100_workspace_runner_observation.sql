ALTER TABLE workspace_runners
    ADD COLUMN IF NOT EXISTS runner_manager_id BIGINT NULL
        REFERENCES runner_managers(runner_manager_id) ON DELETE SET NULL;

ALTER TABLE workspace_runners
    ADD COLUMN IF NOT EXISTS last_observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

-- Existing rows cannot be attributed to a manager and are observation-only data.
DELETE FROM workspace_runners
WHERE runner_manager_id IS NULL;

ALTER TABLE workspace_runners
    DROP CONSTRAINT IF EXISTS workspace_runners_container_name_key;

DROP INDEX IF EXISTS workspace_runners_user_owner_idx;
DROP INDEX IF EXISTS workspace_runners_binding_owner_idx;

CREATE UNIQUE INDEX IF NOT EXISTS workspace_runners_manager_container_idx
    ON workspace_runners (runner_manager_id, container_name);

CREATE UNIQUE INDEX IF NOT EXISTS workspace_runners_manager_user_owner_idx
    ON workspace_runners (runner_manager_id, owner_user_id)
    WHERE owner_kind = 'user';

CREATE UNIQUE INDEX IF NOT EXISTS workspace_runners_manager_binding_owner_idx
    ON workspace_runners (runner_manager_id, owner_workspace_binding_id)
    WHERE owner_kind = 'workspace_binding';

CREATE INDEX IF NOT EXISTS workspace_runners_manager_status_idx
    ON workspace_runners (runner_manager_id, status, last_observed_at);
