-- Binding row lock for one task admission (issue 753 P1).
--
-- Locked after the environment and held until the intent commits, so a racing
-- legacy write waits and then sees the fresh native-active lookup rather than
-- a pre-lock snapshot. Every eligibility fact the caller needs is returned in
-- the same read.
SELECT
    workspace_binding_id,
    application_id,
    external_user_id,
    workspace_key,
    is_active,
    lifecycle_state,
    resource_kind,
    write_lease_owner
FROM workspace_bindings
WHERE workspace_binding_id = $1
FOR UPDATE;
