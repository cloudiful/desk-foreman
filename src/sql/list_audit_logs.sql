SELECT audit_id, actor_user_id, actor_application_id, actor_type, action,
       target_type, target_id, workspace_binding_id, external_user_id, payload,
       request_id, session_id, duration_ms, status, created_at
FROM audit_log
WHERE ($1::TIMESTAMPTZ IS NULL OR created_at >= $1)
  AND ($2::TIMESTAMPTZ IS NULL OR created_at < $2)
  AND ($3::TEXT IS NULL OR action = $3)
  AND ($4::BIGINT IS NULL OR actor_user_id = $4)
  AND ($5::BIGINT IS NULL OR actor_application_id = $5)
  AND ($6::BIGINT IS NULL OR workspace_binding_id = $6)
  AND ($7::BIGINT IS NULL OR session_id = $7)
  AND ($8::TEXT IS NULL OR request_id = $8)
ORDER BY created_at DESC, audit_id DESC
LIMIT $9 OFFSET $10;
