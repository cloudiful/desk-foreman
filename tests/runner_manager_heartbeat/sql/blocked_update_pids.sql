SELECT pid
FROM pg_stat_activity
WHERE datname = current_database()
  AND wait_event_type = 'Lock'
  AND query LIKE '%UPDATE runner_managers%'
  AND query NOT LIKE '%pg_stat_activity%'
ORDER BY pid;
