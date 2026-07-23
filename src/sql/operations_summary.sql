SELECT
    (SELECT COUNT(*) FROM workspace_runners WHERE status = 'running') AS active_runners,
    (SELECT COUNT(*) FROM audit_log
        WHERE created_at >= NOW() - INTERVAL '24 hours'
          AND (status IN ('failed', 'error') OR payload->>'status' IN ('failed', 'error'))) AS failed_operations,
    (SELECT COUNT(*) FROM workspace_bindings WHERE lifecycle_state = 'archived') AS archived_workspaces;
