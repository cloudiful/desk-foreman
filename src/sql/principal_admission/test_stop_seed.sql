INSERT INTO applications (application_id, name) VALUES (817201, 'p4k1');
INSERT INTO workspace_bindings (workspace_binding_id, application_id, external_user_id, workspace_key, external_user_hash, workspace_root, resource_kind, resource_id, write_lease_owner, write_lease_expires_at)
VALUES (817202, 817201, '__resource__', 'project:one', 'hash', '/tmp/p4k1', 'project', 'one', 'current', now() + interval '1 hour'),
       (817203, 817201, '__resource__', 'project:two', 'hash', '/tmp/p4k1', 'project', 'two', 'other', now() + interval '1 hour'),
       (817204, 817201, '__resource__', 'project:three', 'hash', '/tmp/p4k1', 'project', 'three', 'empty', now() - interval '1 hour');
