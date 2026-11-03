-- Scoped count of apply_patch auditing rows for the fixed P2 test principal.
SELECT COUNT(*)::BIGINT
FROM audit_log
WHERE action = 'tool.apply_patch'
  AND target_id = 'patch-fence-test';
