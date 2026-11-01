UPDATE workspace_bindings SET is_active = false, lifecycle_state = 'archived', archived_at = now() WHERE workspace_binding_id = $1;
