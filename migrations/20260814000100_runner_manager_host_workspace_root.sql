ALTER TABLE runner_managers
    ADD COLUMN IF NOT EXISTS host_workspace_root TEXT;
