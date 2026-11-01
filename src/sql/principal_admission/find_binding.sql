SELECT workspace_binding_id, application_id, external_user_id, workspace_key,
       is_active, lifecycle_state, archived_at, resource_kind, resource_id,
       write_lease_owner, write_lease_expires_at > now() AS lease_unexpired
FROM workspace_bindings WHERE workspace_binding_id = $1;
