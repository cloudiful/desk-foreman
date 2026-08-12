UPDATE runner_managers
SET status = 'online', last_seen_at = NOW(), updated_at = NOW()
WHERE runner_manager_id = $1;
