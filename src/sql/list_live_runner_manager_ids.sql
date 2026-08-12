SELECT runner_manager_id
FROM runner_managers
WHERE enabled = TRUE
  AND status = 'online'
  AND last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second');
