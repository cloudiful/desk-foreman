INSERT INTO execution_environments (
    environment_id,
    application_id,
    external_user_id,
    runtime_kind,
    state_volume_ref
)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (application_id, external_user_id) DO NOTHING
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
    updated_at;
