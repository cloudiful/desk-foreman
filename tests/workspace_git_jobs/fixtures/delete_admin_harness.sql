WITH deleted_audit AS (
    DELETE FROM audit_log
    WHERE actor_user_id = $1
)
DELETE FROM users WHERE user_id = $1;
