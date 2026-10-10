-- Is one exact manager still usable as the task's original manager
-- (issue 753 P2b)?
--
-- The manager identity is never reassigned, so preparation first binds to the
-- task's original manager and only then checks that it is still enabled,
-- online and recently seen. A disabled or offline original manager refuses the
-- preparation; it never fails over to another manager.
SELECT runner_manager_id
FROM runner_managers
WHERE runner_manager_id = $1
  AND enabled = TRUE
  AND status = 'online'
  AND last_seen_at > NOW() - ($2::double precision * INTERVAL '1 second')
LIMIT 1;
