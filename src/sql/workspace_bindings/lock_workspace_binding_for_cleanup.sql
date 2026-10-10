-- Lock one workspace binding row for archived-workspace cleanup
-- (issue 753 P1).
--
-- The native-task guard is read AFTER this lock is granted, inside the same
-- READ COMMITTED transaction, so a task admitted while cleanup waited for the
-- row is visible. Held through the filesystem cleanup and the delete so an
-- admitted task can never have its workspace removed underneath it.
SELECT workspace_binding_id
FROM workspace_bindings
WHERE workspace_binding_id = $1
FOR UPDATE;
