SELECT EXISTS(
    SELECT 1
    FROM workspace_bindings
    WHERE workspace_binding_id = $1
)
