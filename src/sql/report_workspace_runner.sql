UPDATE workspace_runners
SET status = $4,
    container_id = COALESCE($5, container_id),
    last_observed_at = NOW(),
    updated_at = NOW(),
    last_error = $6
WHERE runner_manager_id = $1
  AND container_name = $2
  AND owner_kind = $3
  AND (
      (owner_kind = 'user' AND owner_user_id = $7)
      OR
      (owner_kind = 'workspace_binding' AND owner_workspace_binding_id = $8)
      OR
      (owner_kind = 'application_principal' AND owner_workspace_binding_id = $8 AND owner_application_id = $9 AND owner_principal_digest = $10)
  );
