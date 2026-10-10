SELECT
    environment_id,
    application_id,
    external_user_id,
    runtime_kind,
    generation,
    state,
    state_volume_ref,
    last_active_at,
    created_at,
    updated_at
FROM execution_environments
WHERE application_id = $1
  AND external_user_id = $2
LIMIT 1;
