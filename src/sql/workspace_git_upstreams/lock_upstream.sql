SELECT upstream_id
FROM workspace_git_upstreams
WHERE workspace_binding_id = $1
  AND upstream_id = $2
FOR UPDATE
