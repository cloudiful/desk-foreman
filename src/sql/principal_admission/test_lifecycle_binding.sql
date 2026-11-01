INSERT INTO workspace_bindings (workspace_binding_id, application_id, external_user_id, workspace_key, external_user_hash, workspace_root, resource_kind, resource_id, write_lease_owner, write_lease_acquired_at, write_lease_expires_at)
VALUES ($1, 817301, $2, 'workspace', 'hash', $3, 'project', $2, $2, now() - interval '2 hours', now() - interval '1 hour');
