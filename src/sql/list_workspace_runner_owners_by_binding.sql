SELECT DISTINCT owner_application_id, owner_workspace_binding_id, owner_principal_digest
FROM workspace_runners
WHERE owner_kind = 'application_principal' AND owner_workspace_binding_id = $1
ORDER BY owner_application_id, owner_workspace_binding_id, owner_principal_digest;
