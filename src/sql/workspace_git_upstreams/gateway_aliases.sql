SELECT
    upstream_id,
    remote_alias,
    capability_generation
FROM workspace_git_upstreams
WHERE workspace_binding_id = $1
  AND enabled
ORDER BY remote_alias ASC, upstream_id ASC
