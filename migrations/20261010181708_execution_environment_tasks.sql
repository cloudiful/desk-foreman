-- Durable native task ownership and authorized task workspaces (issue 753 P1).
--
-- One durable task intent per authorized workspace binding, owned by an
-- immutable task UUID rather than by an environment generation. No writer,
-- job dispatch, mount, checkout or container exists yet: this phase persists
-- intent and its exclusivity anchors only.
--
-- `operation_token` is the idempotency key, not a credential.
--
-- Additive forward migration only. No prior migration is edited: the
-- `active_native_task_id` / `native_task_id` anchors are future mount-intent
-- pointers, and the existing lifecycle queries select explicit old columns,
-- so no existing route's response shape or stored identity moves.

-- 1. Durable task intent.
CREATE TABLE IF NOT EXISTS execution_environment_tasks (
    task_id UUID PRIMARY KEY,
    operation_token TEXT NOT NULL UNIQUE,
    environment_id UUID NOT NULL,
    application_id BIGINT NOT NULL,
    workspace_binding_id BIGINT NOT NULL
        REFERENCES workspace_bindings (workspace_binding_id) ON DELETE RESTRICT,
    -- Subject audit: the environment already owns the authoritative subject.
    -- Stored for audit only and never compared against the binding marker.
    subject_external_user_id TEXT NOT NULL
        CHECK (length(subject_external_user_id) BETWEEN 1 AND 128),
    target_generation BIGINT NOT NULL CHECK (target_generation >= 0),
    -- Immutable original manager snapshot. A task is never reassigned to a
    -- replacement manager, even if that manager goes offline.
    assigned_manager_id BIGINT NOT NULL
        REFERENCES runner_managers (runner_manager_id) ON DELETE RESTRICT,
    state TEXT NOT NULL DEFAULT 'admitted'
        CHECK (state IN (
            'admitted', 'preparing', 'running', 'stopping', 'uncertain', 'released'
        )),
    -- Candidate container generation. P1 never sets this: it is the mount
    -- intent anchor a later phase writes, and a non-NULL value blocks cancel
    -- from voiding the task.
    container_generation BIGINT NULL
        CHECK (container_generation IS NULL OR container_generation >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    terminal_at TIMESTAMPTZ NULL,
    -- Owner pair must stay a real environment/application pair; the plain
    -- single-column FK alone would accept a foreign pairing.
    CONSTRAINT execution_environment_tasks_environment_application_fk
        FOREIGN KEY (environment_id, application_id)
        REFERENCES execution_environments (environment_id, application_id) ON DELETE CASCADE,
    -- `terminal_at` is set for exactly the terminal state and never for an
    -- in-flight one, so a half-released task is impossible.
    CONSTRAINT execution_environment_tasks_terminal_requires_state CHECK (
        (state = 'released' AND terminal_at IS NOT NULL)
        OR (state <> 'released' AND terminal_at IS NULL)
    ),
    CONSTRAINT execution_environment_tasks_operation_token_bounded CHECK (
        length(operation_token) BETWEEN 1 AND 128
    )
);

-- One active task per environment, across ALL generations. A task owner is a
-- task UUID, never a generation, so an `uncertain` task keeps blocking a new
-- generation's intent rather than being overtaken.
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_tasks_active_environment_key
    ON execution_environment_tasks (environment_id)
    WHERE state IN ('admitted', 'preparing', 'running', 'stopping', 'uncertain');

-- One active task per workspace binding, across all environments: a shared
-- code project workspace may carry at most one task writer.
CREATE UNIQUE INDEX IF NOT EXISTS execution_environment_tasks_active_binding_key
    ON execution_environment_tasks (workspace_binding_id)
    WHERE state IN ('admitted', 'preparing', 'running', 'stopping', 'uncertain');

-- 2. Future mount-intent anchors. Both are nullable typed foreign keys, never
-- JSON parameters, and neither is written by P1.
ALTER TABLE execution_environments
    ADD COLUMN active_native_task_id UUID NULL;

ALTER TABLE execution_environment_jobs
    ADD COLUMN native_task_id UUID NULL;

-- The environment's active task is a real task row, so a dangling pointer is
-- impossible. Added after creation so the table can reference the anchor.
ALTER TABLE execution_environments
    ADD CONSTRAINT execution_environments_active_native_task_fk
        FOREIGN KEY (active_native_task_id)
        REFERENCES execution_environment_tasks (task_id) ON DELETE RESTRICT;

ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_native_task_fk
        FOREIGN KEY (native_task_id)
        REFERENCES execution_environment_tasks (task_id) ON DELETE RESTRICT;

CREATE INDEX IF NOT EXISTS execution_environment_tasks_binding_idx
    ON execution_environment_tasks (workspace_binding_id);
