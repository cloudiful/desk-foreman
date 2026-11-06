-- Optional workspace scope for durable execution environments (issue 1180 P1).
--
-- Source-only, inert foundation: adds a nullable workspace binding scope and
-- partitions identity by scope. Existing rows keep their identity with a NULL
-- scope and are not rewritten.

ALTER TABLE execution_environments
    ADD COLUMN workspace_binding_id BIGINT NULL,
    ADD CONSTRAINT execution_environments_workspace_binding_positive
        CHECK (workspace_binding_id IS NULL OR workspace_binding_id > 0),
    ADD CONSTRAINT execution_environments_workspace_binding_application_fkey
        FOREIGN KEY (workspace_binding_id, application_id)
        REFERENCES workspace_bindings (workspace_binding_id, application_id)
        ON DELETE RESTRICT;

ALTER TABLE execution_environments
    DROP CONSTRAINT execution_environments_identity_key;

CREATE UNIQUE INDEX execution_environments_user_scope_key
    ON execution_environments (application_id, external_user_id)
    WHERE workspace_binding_id IS NULL;

CREATE UNIQUE INDEX execution_environments_binding_scope_key
    ON execution_environments (application_id, external_user_id, workspace_binding_id)
    WHERE workspace_binding_id IS NOT NULL;

CREATE INDEX execution_environments_workspace_binding_id_idx
    ON execution_environments (workspace_binding_id)
    WHERE workspace_binding_id IS NOT NULL;
