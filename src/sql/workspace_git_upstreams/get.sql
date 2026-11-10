SELECT
    upstream_id,
    workspace_binding_id,
    remote_alias,
    target_url,
    enabled,
    (credential_secret IS NOT NULL OR credential_ciphertext IS NOT NULL) AS has_credential,
    capability_generation,
    created_at,
    updated_at
FROM workspace_git_upstreams
WHERE workspace_binding_id = $1
  AND upstream_id = $2
