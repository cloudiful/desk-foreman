SELECT COUNT(*)
FROM workspace_git_upstreams
WHERE workspace_binding_id = $1
