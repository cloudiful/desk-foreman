WITH
    deleted_audit AS (
        DELETE FROM audit_log
        WHERE workspace_binding_id IN (
            SELECT workspace_binding_id
            FROM workspace_bindings
            WHERE application_id = $1
        )
        OR (action = 'workspace.delete' AND target_type = 'workspace_binding' AND target_id = ANY($3::text[]))
    ),
    deleted_jobs AS (
        DELETE FROM workspace_git_jobs WHERE application_id = $1
    ),
    deleted_tasks AS (
        DELETE FROM execution_environment_tasks WHERE application_id = $1
    ),
    deleted_environments AS (
        DELETE FROM execution_environments WHERE application_id = $1
    ),
    deleted_bindings AS (
        DELETE FROM workspace_bindings WHERE application_id = $1
    ),
    deleted_manager AS (
        DELETE FROM runner_managers WHERE runner_manager_id = $2
    )
DELETE FROM applications WHERE application_id = $1;
