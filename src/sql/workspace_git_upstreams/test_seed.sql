INSERT INTO applications (application_id, name)
VALUES (918001, 'p27-upstream-test')
ON CONFLICT (application_id) DO NOTHING;

INSERT INTO workspace_bindings (
    workspace_binding_id,
    application_id,
    external_user_id,
    workspace_key,
    external_user_hash,
    workspace_root
)
VALUES
    (918001, 918001, 'p27-primary', 'default', 'p27-primary-hash', '/tmp/p27-primary'),
    (918002, 918001, 'p27-other', 'default', 'p27-other-hash', '/tmp/p27-other'),
    (918003, 918001, 'p27-archived', 'default', 'p27-archived-hash', '/tmp/p27-archived')
ON CONFLICT (workspace_binding_id) DO NOTHING;

UPDATE workspace_bindings
SET lifecycle_state = 'archived', is_active = FALSE, archived_at = NOW()
WHERE workspace_binding_id = 918003;
