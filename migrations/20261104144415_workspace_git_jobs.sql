-- Durable, secret-free trusted Git operation records (issue 1180 DFI3a1a).
--
-- One row records one binding-scoped sync intent and its capacity reservation.
-- No remote URL, credential, path, userinfo, or raw request is stored. The
-- table is a foundation only: no dispatch, claim endpoint, or runtime caller
-- consumes it yet.

CREATE TABLE IF NOT EXISTS workspace_git_jobs (
    job_id UUID PRIMARY KEY,
    token TEXT NOT NULL UNIQUE,
    operation_id UUID NOT NULL UNIQUE,
    application_id BIGINT NOT NULL,
    workspace_binding_id BIGINT NOT NULL,
    assigned_manager_id BIGINT NOT NULL REFERENCES runner_managers (runner_manager_id) ON DELETE RESTRICT,
    code_project_id UUID NOT NULL,
    authorization_generation UUID NOT NULL,
    job_kind TEXT NOT NULL CHECK (job_kind IN ('sync')),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'claimed', 'succeeded', 'failed', 'uncertain', 'superseded')),
    canonical_branch TEXT NOT NULL CHECK (char_length(canonical_branch) BETWEEN 1 AND 255),
    expected_base TEXT NULL CHECK (
        expected_base IS NULL OR expected_base ~ '^([0-9a-f]{40}|[0-9a-f]{64})$'
    ),
    principal_digest TEXT NOT NULL CHECK (principal_digest ~ '^[0-9a-f]{64}$'),
    lease_owner TEXT NOT NULL CHECK (
        char_length(lease_owner) BETWEEN 1 AND 256 AND lease_owner = btrim(lease_owner)
    ),
    lease_acquired_at TIMESTAMPTZ NOT NULL,
    reserved_bytes BIGINT NOT NULL CHECK (reserved_bytes > 0),
    cleanup_confirmed BOOLEAN NOT NULL DEFAULT FALSE,
    claimed_at TIMESTAMPTZ NULL,
    deadline_at TIMESTAMPTZ NOT NULL,
    terminal_at TIMESTAMPTZ NULL,
    result JSONB NULL CHECK (result IS NULL OR octet_length(result::text) <= 8192),
    error_summary TEXT NULL CHECK (error_summary IS NULL OR char_length(error_summary) <= 1024),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT workspace_git_jobs_job_id_nonzero CHECK (
        job_id <> '00000000-0000-0000-0000-000000000000'::uuid
    ),
    CONSTRAINT workspace_git_jobs_operation_id_nonzero CHECK (
        operation_id <> '00000000-0000-0000-0000-000000000000'::uuid
    ),
    CONSTRAINT workspace_git_jobs_code_project_nonzero CHECK (
        code_project_id <> '00000000-0000-0000-0000-000000000000'::uuid
    ),
    CONSTRAINT workspace_git_jobs_authorization_nonzero CHECK (
        authorization_generation <> '00000000-0000-0000-0000-000000000000'::uuid
    ),
    CONSTRAINT workspace_git_jobs_positive_ids CHECK (
        application_id > 0 AND workspace_binding_id > 0 AND assigned_manager_id > 0
    ),
    CONSTRAINT workspace_git_jobs_token_bounded CHECK (
        char_length(token) BETWEEN 1 AND 128 AND token = btrim(token)
    ),
    CONSTRAINT workspace_git_jobs_binding_application_fk
        FOREIGN KEY (workspace_binding_id, application_id)
        REFERENCES workspace_bindings (workspace_binding_id, application_id)
        ON DELETE RESTRICT,
    CONSTRAINT workspace_git_jobs_active_shape CHECK (
        (state IN ('pending', 'claimed', 'uncertain')
            AND terminal_at IS NULL
            AND cleanup_confirmed = FALSE)
        OR (state IN ('succeeded', 'failed', 'superseded')
            AND terminal_at IS NOT NULL
            AND cleanup_confirmed = TRUE)
    ),
    CONSTRAINT workspace_git_jobs_claimed_shape CHECK (
        (state = 'pending' AND claimed_at IS NULL)
        OR (state IN ('claimed', 'uncertain') AND claimed_at IS NOT NULL)
        OR state IN ('succeeded', 'failed', 'superseded')
    )
);

CREATE INDEX IF NOT EXISTS workspace_git_jobs_assigned_manager_state_created_idx
    ON workspace_git_jobs (assigned_manager_id, state, created_at);

CREATE INDEX IF NOT EXISTS workspace_git_jobs_binding_state_idx
    ON workspace_git_jobs (workspace_binding_id, state);

CREATE UNIQUE INDEX IF NOT EXISTS workspace_git_jobs_active_binding_kind_key
    ON workspace_git_jobs (workspace_binding_id, job_kind)
    WHERE state IN ('pending', 'claimed', 'uncertain');
