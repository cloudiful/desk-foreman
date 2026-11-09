-- Generic exclusive external workspace operations (issue 1180 P24).
--
-- One durable row reserves one workspace binding for one trusted external
-- execution. The row carries no path, credential, remote or business result:
-- only identity, the hashed completion capability, the selected manager and a
-- small state machine. A single active row per binding is what makes the
-- reservation exclusive; a terminal row releases the binding only through an
-- authenticated completion, never through a deadline.

CREATE TABLE IF NOT EXISTS workspace_operations (
    operation_id UUID PRIMARY KEY,
    application_id BIGINT NOT NULL,
    workspace_binding_id BIGINT NOT NULL,
    external_user_id TEXT NOT NULL,
    lease_owner TEXT NOT NULL,
    assigned_manager_id BIGINT NOT NULL REFERENCES runner_managers (runner_manager_id) ON DELETE RESTRICT,
    token_hash TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'active', 'uncertain', 'succeeded', 'failed')),
    deadline_at TIMESTAMPTZ NOT NULL,
    terminal_at TIMESTAMPTZ NULL,
    error TEXT NULL CHECK (
        error IS NULL
        OR error IN (
            'quiescence_failed',
            'quiescence_unknown',
            'activation_failed',
            'completion_conflict'
        )
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT workspace_operations_operation_id_nonzero CHECK (
        operation_id <> '00000000-0000-0000-0000-000000000000'::uuid
    ),
    CONSTRAINT workspace_operations_positive_ids CHECK (
        application_id > 0 AND workspace_binding_id > 0 AND assigned_manager_id > 0
    ),
    CONSTRAINT workspace_operations_external_user_bounded CHECK (
        char_length(external_user_id) BETWEEN 1 AND 128
        AND external_user_id = btrim(external_user_id)
    ),
    CONSTRAINT workspace_operations_lease_owner_bounded CHECK (
        char_length(lease_owner) BETWEEN 1 AND 256 AND lease_owner = btrim(lease_owner)
    ),
    CONSTRAINT workspace_operations_token_hash_bounded CHECK (
        char_length(token_hash) BETWEEN 32 AND 128 AND token_hash = btrim(token_hash)
    ),
    CONSTRAINT workspace_operations_binding_application_fk
        FOREIGN KEY (workspace_binding_id, application_id)
        REFERENCES workspace_bindings (workspace_binding_id, application_id)
        ON DELETE RESTRICT,
    CONSTRAINT workspace_operations_terminal_shape CHECK (
        (state IN ('pending', 'active', 'uncertain') AND terminal_at IS NULL)
        OR (state IN ('succeeded', 'failed') AND terminal_at IS NOT NULL)
    )
);

-- At most one reservation, in any non-terminal state, may own a binding.
CREATE UNIQUE INDEX IF NOT EXISTS workspace_operations_active_binding_key
    ON workspace_operations (workspace_binding_id)
    WHERE state IN ('pending', 'active', 'uncertain');

CREATE INDEX IF NOT EXISTS workspace_operations_assigned_manager_state_idx
    ON workspace_operations (assigned_manager_id, state);

CREATE INDEX IF NOT EXISTS workspace_operations_binding_state_idx
    ON workspace_operations (workspace_binding_id, state);
