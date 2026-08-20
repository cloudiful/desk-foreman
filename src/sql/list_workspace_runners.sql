SELECT
    wr.runner_id,
    wr.runner_manager_id,
    wr.owner_kind,
    wr.owner_user_id,
    wr.owner_workspace_binding_id,
    wr.container_name,
    wr.container_id,
    wr.runtime,
    wr.runtime_class,
    wr.image_name,
    CASE
        WHEN wr.status = 'running'
         AND NOT (
             rm.enabled = TRUE
             AND rm.status = 'online'
             AND rm.last_seen_at > NOW() - ($3::double precision * INTERVAL '1 second')
             AND wr.last_observed_at > NOW() - (($3::double precision * 2) * INTERVAL '1 second')
         ) THEN 'stale'
        ELSE wr.status
    END AS status,
    wr.network_enabled,
    wr.workspace_root,
    wr.last_active_at,
    wr.last_observed_at,
    wr.created_at,
    wr.updated_at,
    wr.last_error
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
  AND ($2::TEXT IS NULL OR wr.owner_kind = $2)
ORDER BY wr.runner_id ASC
LIMIT $4 OFFSET $5;
