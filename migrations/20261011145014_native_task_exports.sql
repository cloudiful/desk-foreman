-- Durable immutable selected-file exports for retained native tasks (issue 774 P1).

ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_job_kind_check;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_job_kind_check CHECK (
        job_kind IN (
            'ensure', 'stop', 'prepare_native_task', 'start_native_task',
            'export_native_task'
        )
    );

ALTER TABLE execution_environment_jobs
    DROP CONSTRAINT execution_environment_jobs_native_kind_requires_task;
ALTER TABLE execution_environment_jobs
    ADD CONSTRAINT execution_environment_jobs_native_kind_requires_task CHECK (
        job_kind NOT IN (
            'prepare_native_task', 'start_native_task', 'export_native_task'
        )
        OR native_task_id IS NOT NULL
    );

CREATE TABLE execution_environment_task_exports (
    job_id UUID PRIMARY KEY
        REFERENCES execution_environment_jobs (job_id) ON DELETE RESTRICT,
    task_id UUID NOT NULL UNIQUE
        REFERENCES execution_environment_tasks (task_id) ON DELETE RESTRICT,
    environment_id UUID NOT NULL,
    application_id BIGINT NOT NULL,
    workspace_binding_id BIGINT NOT NULL
        REFERENCES workspace_bindings (workspace_binding_id) ON DELETE RESTRICT,
    assigned_manager_id BIGINT NOT NULL
        REFERENCES runner_managers (runner_manager_id) ON DELETE RESTRICT,
    target_generation BIGINT NOT NULL CHECK (target_generation >= 0),
    stop_job_id UUID NOT NULL
        REFERENCES execution_environment_jobs (job_id) ON DELETE RESTRICT,
    selection_sha256 TEXT NOT NULL
        CHECK (selection_sha256 ~ '^[0-9a-f]{64}$'),
    selection_file_count INTEGER NOT NULL
        CHECK (selection_file_count BETWEEN 1 AND 4096),
    output_manifest_sha256 TEXT NULL
        CHECK (output_manifest_sha256 IS NULL OR output_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    output_file_count INTEGER NULL CHECK (output_file_count BETWEEN 0 AND 4096),
    output_missing_count INTEGER NULL CHECK (output_missing_count BETWEEN 0 AND 4096),
    output_total_bytes BIGINT NULL CHECK (output_total_bytes BETWEEN 0 AND 8388608),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ NULL,
    CONSTRAINT execution_environment_task_exports_environment_application_fk
        FOREIGN KEY (environment_id, application_id)
        REFERENCES execution_environments (environment_id, application_id) ON DELETE RESTRICT,
    CONSTRAINT execution_environment_task_exports_output_shape CHECK (
        (output_manifest_sha256 IS NULL
            AND output_file_count IS NULL
            AND output_missing_count IS NULL
            AND output_total_bytes IS NULL
            AND completed_at IS NULL)
        OR (output_manifest_sha256 IS NOT NULL
            AND output_file_count IS NOT NULL
            AND output_missing_count IS NOT NULL
            AND output_total_bytes IS NOT NULL
            AND completed_at IS NOT NULL
            AND output_file_count + output_missing_count = selection_file_count)
    )
);

CREATE INDEX execution_environment_task_exports_stop_job_idx
    ON execution_environment_task_exports (stop_job_id);

CREATE TABLE execution_environment_task_export_selection_files (
    job_id UUID NOT NULL
        REFERENCES execution_environment_task_exports (job_id) ON DELETE RESTRICT,
    path TEXT NOT NULL CHECK (octet_length(path) BETWEEN 1 AND 512),
    PRIMARY KEY (job_id, path)
);

CREATE TABLE execution_environment_task_export_files (
    job_id UUID NOT NULL,
    path TEXT NOT NULL,
    content TEXT NOT NULL CHECK (octet_length(content) <= 1048576),
    PRIMARY KEY (job_id, path),
    FOREIGN KEY (job_id, path)
        REFERENCES execution_environment_task_export_selection_files (job_id, path)
        ON DELETE RESTRICT
);

CREATE TABLE execution_environment_task_export_missing_paths (
    job_id UUID NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (job_id, path),
    FOREIGN KEY (job_id, path)
        REFERENCES execution_environment_task_export_selection_files (job_id, path)
        ON DELETE RESTRICT
);
