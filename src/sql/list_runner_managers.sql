SELECT
    runner_manager_id,
    name,
    endpoint,
    enabled,
    image,
    network_enabled,
    max_output_bytes,
    max_timeout_ms,
    max_sessions,
    pids_limit,
    memory_limit,
    cpu_limit,
    host_workspace_root,
    CASE
        WHEN enabled = TRUE
             AND status = 'online'
             AND last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second')
        THEN status
        ELSE 'offline'
    END AS status,
    last_seen_at,
    created_at,
    updated_at
FROM runner_managers
WHERE ($2::TEXT IS NULL OR name ILIKE '%' || $2 || '%')
  AND ($3::BOOLEAN IS NULL OR enabled = $3)
ORDER BY runner_manager_id ASC
LIMIT $4 OFFSET $5;
