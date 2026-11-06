INSERT INTO execution_environments (
    environment_id,
    application_id,
    external_user_id,
    runtime_kind,
    state_volume_ref,
    workspace_binding_id,
    state
)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT DO NOTHING
RETURNING
    environment_id,
    application_id,
    external_user_id,
    runtime_kind,
    generation,
    state,
    state_volume_ref,
    last_active_at,
    created_at,
    updated_at,
    workspace_binding_id;
