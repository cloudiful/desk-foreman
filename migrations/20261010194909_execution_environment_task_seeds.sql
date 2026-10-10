-- Immutable authorized task input snapshot (issue 753 P2a).
--
-- One sealed snapshot of the initial files an authenticated application
-- authorized for an admitted task. The snapshot is immutable: there is no
-- update or delete path, and no later phase may overwrite it in place.
--
-- The manifest is a content hash over the file bytes only. It deliberately
-- excludes the task id and the authorization generation, so the same content
-- hashes identically across tasks and generations and can back an explicit
-- content CAS against a canonical project later.
--
-- Additive forward migration only: no prior migration is edited, and the
-- existing environment/job/task tables are untouched.

-- 1. One seed header per task.
CREATE TABLE IF NOT EXISTS execution_environment_task_seeds (
    task_id UUID PRIMARY KEY
        REFERENCES execution_environment_tasks (task_id) ON DELETE RESTRICT,
    -- Lower-case hex SHA-256 over the versioned manifest bytes.
    manifest_sha256 TEXT NOT NULL
        CHECK (manifest_sha256 ~ '^[0-9a-f]{64}$'),
    -- Entry and byte ceilings mirrored by the application-side bounds.
    file_count BIGINT NOT NULL CHECK (file_count BETWEEN 0 AND 4096),
    total_bytes BIGINT NOT NULL CHECK (total_bytes BETWEEN 0 AND 8388608),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. The sealed file contents, one row per path.
--
-- `(task_id, path)` is the primary key, so a duplicate path is impossible at
-- the storage layer even if an application-side check ever missed one. Rows
-- are inserted in path order so the stored ordering matches the manifest.
CREATE TABLE IF NOT EXISTS execution_environment_task_seed_files (
    task_id UUID NOT NULL
        REFERENCES execution_environment_task_seeds (task_id) ON DELETE RESTRICT,
    path TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY (task_id, path)
);
