-- Fresh native-active task check for one workspace binding (issue 753 P1).
--
-- Run inside an already-open transaction that holds the binding row lock and
-- read again after the lock is granted, so the answer reflects committed
-- state rather than a snapshot taken before a concurrent admission committed.
-- Returns the single active task id; `state = ANY(...)` keeps the active set in
-- one place with the partial unique indexes.
SELECT task_id
FROM execution_environment_tasks
WHERE workspace_binding_id = $1
  AND state IN ('admitted', 'preparing', 'prepared', 'starting', 'running', 'stopping', 'uncertain')
LIMIT 1;
