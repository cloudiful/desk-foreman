UPDATE workspace_bindings
SET
    last_used_at = NOW(),
    updated_at = NOW()
WHERE workspace_binding_id = $1;
