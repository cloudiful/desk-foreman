SELECT COUNT(*)::BIGINT AS count
FROM workspace_runners wr
LEFT JOIN runner_managers rm ON rm.runner_manager_id = wr.runner_manager_id
WHERE (
        $1::TEXT IS NULL
        OR CASE
            WHEN wr.status = 'running'
             AND NOT (
                 rm.enabled = TRUE
                 AND rm.status = 'online'
                 AND rm.last_seen_at > NOW() - ($3::double precision * INTERVAL '1 second')
                 AND wr.last_observed_at > NOW() - (($3::double precision * 2) * INTERVAL '1 second')
             ) THEN 'stale'
            ELSE wr.status
           END = $1
      )
  AND ($2::TEXT IS NULL OR wr.owner_kind = $2);
