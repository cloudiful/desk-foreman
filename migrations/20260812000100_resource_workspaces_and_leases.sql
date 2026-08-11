-- Resource-owned shared workspaces and write leases.
--
-- Resource bindings (e.g. workspace_key = "code_project:<id>") are shared
-- across external users of an application. They use a fixed marker
-- external_user_id so the existing (application_id, external_user_id,
-- workspace_key) uniqueness keeps a single row per resource.
--
-- The write lease gates mutating tools (shell, apply_patch, write_stdin) so
-- only one AI session at a time can modify a shared resource workspace.
-- Read-only tools are unaffected.

ALTER TABLE workspace_bindings
    ADD COLUMN resource_kind TEXT NULL,
    ADD COLUMN resource_id TEXT NULL,
    ADD COLUMN write_lease_owner TEXT NULL,
    ADD COLUMN write_lease_acquired_at TIMESTAMPTZ NULL,
    ADD COLUMN write_lease_expires_at TIMESTAMPTZ NULL;

CREATE UNIQUE INDEX workspace_bindings_resource_identity_idx
    ON workspace_bindings (application_id, resource_kind, resource_id)
    WHERE resource_kind IS NOT NULL AND resource_id IS NOT NULL;
