ALTER TABLE workspace_runners
    ADD COLUMN owner_epoch BIGINT NULL
    CONSTRAINT workspace_runners_owner_epoch_positive CHECK (owner_epoch IS NULL OR owner_epoch > 0);

ALTER TABLE runner_principal_admission
    ADD COLUMN owner_epoch BIGINT NULL
    CONSTRAINT runner_principal_admission_owner_epoch_positive CHECK (owner_epoch IS NULL OR owner_epoch > 0);

ALTER TABLE runner_owner_stop_fence
    ADD COLUMN owner_epoch BIGINT NULL
    CONSTRAINT runner_owner_stop_fence_owner_epoch_positive CHECK (owner_epoch IS NULL OR owner_epoch > 0);

ALTER TABLE workspace_git_jobs
    ADD COLUMN owner_epoch BIGINT NULL
    CONSTRAINT workspace_git_jobs_owner_epoch_positive CHECK (owner_epoch IS NULL OR owner_epoch > 0);
