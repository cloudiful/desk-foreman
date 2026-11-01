UPDATE workspace_bindings SET lifecycle_state = 'archived', is_active = false, archived_at = now() - interval '400 days' WHERE workspace_binding_id = $1;
