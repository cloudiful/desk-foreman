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
WHERE access_token_hash = $1
LIMIT 1;
