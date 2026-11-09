SELECT lifecycle_state, is_active
FROM workspace_bindings
WHERE workspace_binding_id = $1
FOR UPDATE
