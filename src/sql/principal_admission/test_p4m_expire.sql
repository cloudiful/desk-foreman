UPDATE workspace_bindings SET write_lease_expires_at = now() - interval '1 minute' WHERE workspace_binding_id = $1;
