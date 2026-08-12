SELECT
    runner_manager_id,
    name,
    endpoint,
    access_token_hash,
    enabled,
    image,
    network_enabled,
    max_output_bytes,
    max_timeout_ms,
    max_sessions,
    pids_limit,
    memory_limit,
    cpu_limit,
    status,
    last_seen_at,
    created_at,
    updated_at
FROM runner_managers
WHERE enabled = TRUE
  AND status = 'online'
  AND last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second')
ORDER BY last_seen_at DESC NULLS LAST, runner_manager_id ASC
LIMIT 1;
