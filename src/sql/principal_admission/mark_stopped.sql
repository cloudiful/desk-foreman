UPDATE runner_principal_admission SET stopped_at = now()
WHERE owner_key = $1 AND application_id = $2 AND workspace_binding_id = $3 AND principal_digest = $4
  AND runner_manager_id = $5 AND container_id = $6 AND stopped_at IS NULL;
