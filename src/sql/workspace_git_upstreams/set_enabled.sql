UPDATE workspace_git_upstreams
SET enabled = $3,
    capability_generation = capability_generation + 1,
    updated_at = NOW()
WHERE workspace_binding_id = $1
  AND upstream_id = $2
RETURNING
    upstream_id,
    workspace_binding_id,
    remote_alias,
    target_url,
    enabled,
    (credential_ciphertext IS NOT NULL) AS has_credential,
    capability_generation,
    created_at,
    updated_at
