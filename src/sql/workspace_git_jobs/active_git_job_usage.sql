-- Capacity usage for the global, one-binding, and one-manager scopes.
-- Only pending, claimed, and uncertain states reserve capacity.
WITH active AS (
    SELECT workspace_binding_id, assigned_manager_id, reserved_bytes
    FROM workspace_git_jobs
    WHERE state IN ('pending', 'claimed', 'uncertain')
)
SELECT
    COUNT(*) AS global_count,
    COALESCE(SUM(reserved_bytes), 0)::BIGINT AS global_bytes,
    COUNT(*) FILTER (WHERE workspace_binding_id = $1) AS binding_count,
    COALESCE(SUM(reserved_bytes) FILTER (WHERE workspace_binding_id = $1), 0)::BIGINT AS binding_bytes,
    COUNT(*) FILTER (WHERE assigned_manager_id = $2) AS manager_count,
    COALESCE(SUM(reserved_bytes) FILTER (WHERE assigned_manager_id = $2), 0)::BIGINT AS manager_bytes
FROM active;
