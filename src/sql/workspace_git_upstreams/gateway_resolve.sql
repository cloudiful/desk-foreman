SELECT
    u.upstream_id,
    u.workspace_binding_id,
    u.remote_alias,
    u.target_url,
    u.credential_username,
    u.credential_secret,
    u.credential_ciphertext,
    u.enabled,
    u.capability_generation,
    b.application_id,
    a.is_active AS application_active
FROM workspace_git_upstreams u
JOIN workspace_bindings b
    ON b.workspace_binding_id = u.workspace_binding_id
JOIN applications a
    ON a.application_id = b.application_id
WHERE u.workspace_binding_id = $1
  AND u.remote_alias = $2
