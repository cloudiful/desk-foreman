-- Fresh scoped-environment busy check for one locked workspace binding
-- (issue 1180 P2).
--
-- The single generic guard every workspace write, lifecycle and lease path
-- uses. It must run inside the caller's already-open transaction that holds
-- the workspace binding row lock, and must be re-read AFTER that lock is
-- granted: an environment anchor established while we waited on the lock
-- would be invisible to a snapshot taken before it.
--
-- `state IN (...)` is the exact busy set. `absent` and `replaced` never block;
-- `starting`, `ready`, `stopping`, `unreachable` and `uncertain` do, because
-- each still means a scoped helper may own this workspace.
SELECT environment_id
FROM execution_environments
WHERE workspace_binding_id = $1
  AND state IN ('starting', 'ready', 'stopping', 'unreachable', 'uncertain')
LIMIT 1;
