UPDATE workspace_git_upstreams
SET credential_username = $3,
    credential_secret = $4,
    credential_ciphertext = NULL,
    credential_updated_at = NOW()
WHERE workspace_binding_id = $1
  AND upstream_id = $2
