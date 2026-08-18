SELECT audit_id, actor_user_id, actor_application_id, actor_type, action,
       target_type, target_id, workspace_binding_id, external_user_id, payload,
       request_id, session_id, duration_ms, status, created_at
FROM audit_log
WHERE ($1::TIMESTAMPTZ IS NULL OR created_at >= $1)
   AND ($2::TIMESTAMPTZ IS NULL OR created_at < $2)
   AND ($3::TEXT IS NULL OR action = $3)
   AND ($4::TEXT IS NULL OR action ILIKE '%' || $4 || '%' OR actor_type ILIKE '%' || $4 || '%' OR target_type ILIKE '%' || $4 || '%')
   AND ($5::TEXT IS NULL OR ($5 = 'success' AND status = 'success') OR ($5 = 'failure' AND status IS NOT NULL AND status <> 'success') OR ($5 = 'unknown' AND (status IS NULL OR status = 'unknown')))
   AND ($6::BIGINT IS NULL OR actor_user_id = $6)
   AND ($7::BIGINT IS NULL OR actor_application_id = $7)
   AND ($8::BIGINT IS NULL OR workspace_binding_id = $8)
   AND ($9::BIGINT IS NULL OR session_id = $9)
   AND ($10::TEXT IS NULL OR request_id = $10)
ORDER BY created_at DESC, audit_id DESC
LIMIT $11 OFFSET $12;
