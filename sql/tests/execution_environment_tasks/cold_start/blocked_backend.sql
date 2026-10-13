-- Positive observation of the blocked cold-start backend (issue 824 P1r).
--
-- Returns one row only when the exact private database has a backend with the
-- expected application name waiting, ungranted, on this advisory lock. Polling
-- for this row is what proves the start transaction really reached the gated
-- environment update; an absent row means the precondition has not been met
-- yet, never that it never will.

SELECT count(*)::BIGINT AS blocked_backends
FROM pg_locks AS locks
JOIN pg_stat_activity AS activity ON activity.pid = locks.pid
WHERE locks.locktype = 'advisory'
  AND locks.granted = FALSE
  AND locks.objid = 824000001
  AND activity.application_name = $1
  AND activity.datname = current_database();
