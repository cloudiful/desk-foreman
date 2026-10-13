-- Quiescence of the aborted cold-start backend (issue 824 P1r).
--
-- One row only when the exact backend is idle, holds no transaction, and no
-- longer waits on this advisory lock. That is the state the rolled-back start
-- transaction must reach once the gate is released and SQLx has dropped the
-- aborted connection's future work.

SELECT count(*)::BIGINT AS still_busy
FROM pg_stat_activity AS activity
WHERE activity.application_name = $1
  AND activity.datname = current_database()
  AND activity.pid <> pg_backend_pid()
  AND (
      activity.state <> 'idle'
      OR activity.xact_start IS NOT NULL
      OR EXISTS (
          SELECT 1
          FROM pg_locks AS locks
          WHERE locks.pid = activity.pid
            AND locks.locktype = 'advisory'
            AND locks.granted = FALSE
            AND locks.objid = 824000001
      )
  );
