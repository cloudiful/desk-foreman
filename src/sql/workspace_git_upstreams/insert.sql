INSERT INTO workspace_git_upstreams (workspace_binding_id, remote_alias, target_url)
VALUES ($1, $2, $3)
RETURNING upstream_id
