-- Idempotent fixture for the P2 patch fence/audit tests. The binding holds a
-- live write lease for the fixed patch-test principal. Data is reset by
-- test_patch_cleanup.sql before each test and uses only the owned test DB.
INSERT INTO applications (application_id, name, is_active, created_at, updated_at)
VALUES (991001, 'p2-patch-fence-fixture', TRUE, NOW(), NOW())
ON CONFLICT (application_id) DO UPDATE
SET name = EXCLUDED.name, is_active = TRUE, updated_at = NOW();

INSERT INTO workspace_bindings (
    workspace_binding_id,
    application_id,
    external_user_id,
    workspace_key,
    external_user_hash,
    workspace_root,
    is_active,
    last_used_at,
    created_at,
    updated_at,
    lifecycle_state,
    archived_at,
    resource_kind,
    resource_id,
    write_lease_owner,
    write_lease_acquired_at,
    write_lease_expires_at
)
VALUES (
    991002,
    991001,
    '__resource__',
    'code_project:p2-patch-fence',
    'p2-patch-fence-hash',
    '/tmp/df-patch-fence-fixture',
    TRUE,
    NOW(),
    NOW(),
    NOW(),
    'active',
    NULL,
    'code_project',
    'p2-patch-fence',
    'patch-fence-test',
    NOW(),
    NOW() + INTERVAL '10 minutes'
)
ON CONFLICT (workspace_binding_id) DO UPDATE
SET application_id = EXCLUDED.application_id,
    external_user_id = EXCLUDED.external_user_id,
    workspace_key = EXCLUDED.workspace_key,
    external_user_hash = EXCLUDED.external_user_hash,
    workspace_root = EXCLUDED.workspace_root,
    is_active = TRUE,
    last_used_at = NOW(),
    updated_at = NOW(),
    lifecycle_state = 'active',
    archived_at = NULL,
    resource_kind = EXCLUDED.resource_kind,
    resource_id = EXCLUDED.resource_id,
    write_lease_owner = EXCLUDED.write_lease_owner,
    write_lease_acquired_at = NOW(),
    write_lease_expires_at = NOW() + INTERVAL '10 minutes';
