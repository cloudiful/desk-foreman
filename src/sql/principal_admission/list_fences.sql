SELECT owner_key, application_id, workspace_binding_id, principal_digest, requested_at
FROM runner_owner_stop_fence WHERE workspace_binding_id = $1 ORDER BY owner_key;
