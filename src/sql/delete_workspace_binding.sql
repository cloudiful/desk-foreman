DELETE FROM workspace_bindings
WHERE workspace_binding_id = $1
RETURNING workspace_root;
