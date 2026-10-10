-- Fresh native-active task check for one locked workspace binding
-- (issue 753 P1).
--
-- The single guard query every legacy writer uses. It must be run inside the
-- caller's already-open transaction that holds the workspace binding row lock,
-- and must be re-read AFTER that lock is granted: a `NOT EXISTS` evaluated in
-- the snapshot taken before the lock is exactly the interleaving that would
-- let an admitted task be written through by a legacy path.
--
-- `state IN (...)` matches the partial unique indexes that define "active".
SELECT task_id
FROM execution_environment_tasks
WHERE workspace_binding_id = $1
  AND state IN ('admitted', 'preparing', 'prepared', 'running', 'stopping', 'uncertain')
LIMIT 1;
