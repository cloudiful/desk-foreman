-- Remove only the fixed P2 patch fixture rows and scoped audit entries.
DELETE FROM audit_log
WHERE action = 'tool.apply_patch'
  AND target_id = 'patch-fence-test';

DELETE FROM workspace_bindings
WHERE workspace_binding_id = 991002;

DELETE FROM applications
WHERE application_id = 991001;
