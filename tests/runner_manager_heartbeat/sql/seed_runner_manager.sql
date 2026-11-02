UPDATE runner_managers
SET enabled = $2, status = $3, last_seen_at = $4, updated_at = $5
WHERE runner_manager_id = $1
RETURNING status, enabled, last_seen_at, updated_at;
