UPDATE execution_environments
SET
    generation = generation + 1,
    state = 'stopping',
    last_active_at = NOW(),
    updated_at = NOW()
WHERE environment_id = $1
  AND application_id = $2
  AND generation = $3
  AND active_native_task_id IS NULL
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
