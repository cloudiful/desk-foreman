SELECT
    remote_alias,
    target_url,
    credential_username,
    credential_secret,
    credential_ciphertext
FROM workspace_git_upstreams
WHERE workspace_binding_id = $1
  AND upstream_id = $2
FOR UPDATE
