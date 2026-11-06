SELECT
    EXISTS (SELECT 1 FROM audit_log WHERE workspace_binding_id = $1)
    OR EXISTS (SELECT 1 FROM execution_environment_tasks WHERE workspace_binding_id = $1)
    OR EXISTS (SELECT 1 FROM execution_environment_task_exports WHERE workspace_binding_id = $1)
    OR EXISTS (SELECT 1 FROM execution_environments WHERE workspace_binding_id = $1) AS "has_references!";
