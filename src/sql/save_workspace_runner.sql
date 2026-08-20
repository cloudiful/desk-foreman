INSERT INTO workspace_runners (
    runner_manager_id,
    owner_kind,
    owner_user_id,
    owner_workspace_binding_id,
    container_name,
    container_id,
    runtime,
    runtime_class,
    image_name,
    status,
    network_enabled,
    workspace_root,
    last_active_at,
    updated_at,
    last_observed_at,
    last_error
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
    NOW(), NOW(), NOW(), $13
)
ON CONFLICT (runner_manager_id, container_name) DO UPDATE
SET
    runner_manager_id = EXCLUDED.runner_manager_id,
    owner_kind = EXCLUDED.owner_kind,
    owner_user_id = EXCLUDED.owner_user_id,
    owner_workspace_binding_id = EXCLUDED.owner_workspace_binding_id,
    container_id = EXCLUDED.container_id,
    runtime = EXCLUDED.runtime,
    runtime_class = EXCLUDED.runtime_class,
    image_name = EXCLUDED.image_name,
    status = EXCLUDED.status,
    network_enabled = EXCLUDED.network_enabled,
    workspace_root = EXCLUDED.workspace_root,
    last_active_at = NOW(),
    updated_at = NOW(),
    last_observed_at = NOW(),
    last_error = EXCLUDED.last_error
RETURNING *;
