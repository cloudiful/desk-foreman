SELECT
    (SELECT COUNT(*)
     FROM workspace_runners wr
     JOIN runner_managers rm ON rm.runner_manager_id = wr.runner_manager_id
     WHERE wr.status = 'running'
       AND rm.enabled = TRUE
       AND rm.status = 'online'
       AND rm.last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second')
       AND wr.last_observed_at > NOW() - (($1::double precision * 2) * INTERVAL '1 second')) AS active_runners,
    (SELECT COUNT(*) FROM audit_log
        WHERE created_at >= NOW() - INTERVAL '24 hours'
          AND (status IN ('failed', 'error') OR payload->>'status' IN ('failed', 'error'))) AS failed_operations,
    (SELECT COUNT(*) FROM workspace_bindings WHERE lifecycle_state = 'archived') AS archived_workspaces,
    (SELECT COUNT(*) FROM runner_managers) AS runner_managers_total,
    (SELECT COUNT(*) FROM runner_managers
        WHERE enabled = TRUE
          AND status = 'online'
          AND last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second')) AS runner_managers_online,
    (SELECT COUNT(*) FROM runner_managers
        WHERE enabled = TRUE
          AND NOT (
              status = 'online'
              AND last_seen_at IS NOT NULL
              AND last_seen_at > NOW() - ($1::double precision * INTERVAL '1 second')
          )) AS runner_managers_offline,
    (SELECT COUNT(*) FROM runner_managers WHERE enabled = FALSE) AS runner_managers_disabled;
