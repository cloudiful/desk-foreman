-- Lock one workspace binding row for an admin lifecycle transition
-- (issue 753 P1).
--
-- The native-task guard is read AFTER this lock is granted, inside the same
-- READ COMMITTED transaction, so a task admitted while the transition waited
-- for the row is visible. Taken before any state change so the transition's
-- decision and its commit are serialized against task admission.
SELECT application_id
FROM workspace_bindings
WHERE workspace_binding_id = $1
FOR UPDATE;
