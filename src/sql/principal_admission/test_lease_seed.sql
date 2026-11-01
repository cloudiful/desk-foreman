INSERT INTO applications (application_id, name) VALUES (817401, 'p4k3');
INSERT INTO workspace_bindings (workspace_binding_id, application_id, external_user_id, workspace_key, external_user_hash, workspace_root, resource_kind, resource_id, write_lease_owner, write_lease_acquired_at, write_lease_expires_at)
VALUES (817402, 817401, 'old', 'workspace', 'hash', '/tmp/p4k3', 'project', 'p4k3', 'old', now() - interval '2 hours', now() + interval '1 hour'),
       (817403, 817401, 'other', 'workspace', 'hash', '/tmp/p4k3', 'project', 'p4k3-other', NULL, NULL, NULL);
