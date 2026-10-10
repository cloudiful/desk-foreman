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
WHERE environment_id = $1
  AND application_id = $2
LIMIT 1;
