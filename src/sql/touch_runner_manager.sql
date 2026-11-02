UPDATE runner_managers
SET status = 'online', last_seen_at = NOW(), updated_at = NOW()
WHERE runner_manager_id = $1
  AND (
      status <> 'online'
      OR last_seen_at IS NULL
      OR last_seen_at <= NOW() - ($2::double precision * INTERVAL '1 second')
  );
