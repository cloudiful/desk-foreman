UPDATE runner_managers
SET
    endpoint = $2,
    enabled = $3,
    image = $4,
    network_enabled = $5,
    max_output_bytes = $6,
    max_timeout_ms = $7,
    max_sessions = $8,
    pids_limit = $9,
    memory_limit = $10,
    cpu_limit = $11,
    host_workspace_root = $12,
    updated_at = NOW()
WHERE runner_manager_id = $1
RETURNING
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
    status,
    last_seen_at,
    created_at,
    updated_at;
