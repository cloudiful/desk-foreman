-- Potential-writer observations for one bound workspace scope
-- (issue 1180 P9).
--
-- Every non-removed observation of this binding is retained, including stale,
-- offline, disabled, cleanup-failed, unknown and orphaned manager rows. They
-- are grouped per observed manager and root so positive admission requires one
-- group whose every row is simultaneously running, freshly observed and backed
-- by an enabled/online/fresh manager. Stale heartbeat is never quiescence
-- proof: it only makes the group non-authorizing while still blocking through
-- its presence. Removed rows are ignored only under the existing per-container
-- stop/rm report contract.
SELECT
    wr.runner_manager_id,
    wr.workspace_root,
    COALESCE(
        BOOL_AND(
            wr.status = 'running'
            AND wr.last_observed_at > NOW() - ((2 * $2::bigint) * INTERVAL '1 second')
            AND rm.enabled = TRUE
            AND rm.status = 'online'
            AND rm.last_seen_at > NOW() - ($2::bigint * INTERVAL '1 second')
        ),
        FALSE
    ) AS "authorizes!"
FROM workspace_runners wr
LEFT JOIN runner_managers rm ON rm.runner_manager_id = wr.runner_manager_id
WHERE wr.owner_workspace_binding_id = $1
  AND wr.status IS DISTINCT FROM 'removed'
GROUP BY wr.runner_manager_id, wr.workspace_root
ORDER BY wr.runner_manager_id NULLS FIRST, wr.workspace_root
LIMIT 2;
