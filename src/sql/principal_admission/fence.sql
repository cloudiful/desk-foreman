INSERT INTO runner_owner_stop_fence (owner_key, application_id, workspace_binding_id, principal_digest)
VALUES ($1, $2, $3, $4)
ON CONFLICT (owner_key) DO NOTHING;
