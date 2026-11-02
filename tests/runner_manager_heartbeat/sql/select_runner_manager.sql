SELECT status, enabled, last_seen_at, updated_at
FROM runner_managers
WHERE runner_manager_id = $1;
