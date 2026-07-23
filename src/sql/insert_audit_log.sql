INSERT INTO audit_log (
    actor_user_id,
    actor_application_id,
    actor_type,
    action,
    target_type,
    target_id,
    workspace_binding_id,
    external_user_id,
    payload,
    request_id,
    session_id,
    duration_ms,
    status
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13);
