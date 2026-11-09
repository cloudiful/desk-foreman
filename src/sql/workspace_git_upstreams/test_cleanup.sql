DELETE FROM workspace_git_upstreams WHERE workspace_binding_id IN (918001, 918002, 918003);
DELETE FROM workspace_bindings WHERE workspace_binding_id IN (918001, 918002, 918003);
DELETE FROM applications WHERE application_id = 918001;
