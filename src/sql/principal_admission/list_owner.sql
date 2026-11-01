SELECT owner_key, application_id, workspace_binding_id, principal_digest, runner_manager_id, container_id, admitted_at, stopped_at
FROM runner_principal_admission WHERE owner_key = $1 AND stopped_at IS NULL
ORDER BY runner_manager_id, container_id;
