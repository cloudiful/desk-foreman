-- Bound the set of live managers already observed owning this binding's
-- application principal. The result is at most two distinct manager ids so a
-- caller can distinguish one exact target from a stale/ambiguous observation.
SELECT DISTINCT rm.runner_manager_id
FROM workspace_runners wr
JOIN runner_managers rm ON rm.runner_manager_id = wr.runner_manager_id
WHERE wr.owner_workspace_binding_id = $1
  AND wr.owner_kind = 'application_principal'
  AND wr.status = 'running'
  AND rm.enabled = TRUE
  AND rm.status = 'online'
  AND rm.last_seen_at > NOW() - ($2::double precision * INTERVAL '1 second')
  AND wr.last_observed_at > NOW() - (($2::double precision * 2) * INTERVAL '1 second')
ORDER BY rm.runner_manager_id
LIMIT 2;
