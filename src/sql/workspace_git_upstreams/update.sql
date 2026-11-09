UPDATE workspace_git_upstreams
SET remote_alias = COALESCE($3::text, remote_alias),
    target_url = COALESCE($4::text, target_url),
    credential_username = CASE
        WHEN $5::boolean THEN NULL
        WHEN $6::text IS NOT NULL THEN $6::text
        ELSE credential_username
    END,
    credential_ciphertext = CASE
        WHEN $5::boolean THEN NULL
        WHEN $7::bytea IS NOT NULL THEN $7::bytea
        ELSE credential_ciphertext
    END,
    credential_updated_at = CASE
        WHEN $5::boolean THEN NULL
        WHEN $6::text IS NOT NULL THEN NOW()
        ELSE credential_updated_at
    END,
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
