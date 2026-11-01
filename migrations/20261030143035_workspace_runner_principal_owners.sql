ALTER TABLE workspace_bindings
    ADD CONSTRAINT workspace_bindings_id_application_key UNIQUE (workspace_binding_id, application_id);

ALTER TABLE workspace_runners
    ADD COLUMN owner_application_id BIGINT,
    ADD COLUMN owner_principal_digest TEXT,
    DROP CONSTRAINT workspace_runners_owner_check,
    ADD CONSTRAINT workspace_runners_owner_check CHECK (
        (owner_kind = 'user' AND owner_user_id IS NOT NULL AND owner_workspace_binding_id IS NULL AND owner_application_id IS NULL AND owner_principal_digest IS NULL)
        OR (owner_kind = 'workspace_binding' AND owner_user_id IS NULL AND owner_workspace_binding_id IS NOT NULL AND owner_application_id IS NULL AND owner_principal_digest IS NULL)
        OR (owner_kind = 'application_principal' AND owner_user_id IS NULL AND owner_workspace_binding_id IS NOT NULL AND owner_workspace_binding_id > 0 AND owner_application_id IS NOT NULL AND owner_application_id > 0 AND owner_principal_digest IS NOT NULL AND owner_principal_digest ~ '^[0-9a-f]{64}$')
    ),
    ADD CONSTRAINT workspace_runners_binding_application_fkey FOREIGN KEY (owner_workspace_binding_id, owner_application_id)
        REFERENCES workspace_bindings (workspace_binding_id, application_id) ON DELETE CASCADE;

CREATE UNIQUE INDEX workspace_runners_manager_principal_owner_idx
    ON workspace_runners (runner_manager_id, owner_application_id, owner_workspace_binding_id, owner_principal_digest)
    WHERE owner_kind = 'application_principal';
