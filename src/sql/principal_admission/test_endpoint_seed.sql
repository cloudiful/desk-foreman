INSERT INTO applications (application_id, name) VALUES (817101, 'p4j2');
INSERT INTO workspace_bindings (workspace_binding_id, application_id, external_user_id, workspace_key, external_user_hash, workspace_root, resource_kind, resource_id, write_lease_owner, write_lease_expires_at)
VALUES (817102, 817101, '__resource__', 'workspace', 'hash', '/tmp/p4j2', 'project', 'one', 'principal', now() + interval '1 hour');
