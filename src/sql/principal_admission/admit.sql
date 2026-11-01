INSERT INTO runner_principal_admission (owner_key, application_id, workspace_binding_id, principal_digest, runner_manager_id, container_id)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT (runner_manager_id, owner_key, container_id) DO UPDATE SET owner_key = EXCLUDED.owner_key
WHERE runner_principal_admission.stopped_at IS NULL
RETURNING owner_key, application_id, workspace_binding_id, principal_digest, runner_manager_id, container_id, admitted_at, stopped_at;
