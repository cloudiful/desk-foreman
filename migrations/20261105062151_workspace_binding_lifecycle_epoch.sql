ALTER TABLE workspace_bindings
    ADD COLUMN lifecycle_epoch BIGINT NOT NULL DEFAULT 1
    CONSTRAINT workspace_bindings_lifecycle_epoch_positive CHECK (lifecycle_epoch > 0);
