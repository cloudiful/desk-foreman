SELECT COUNT(*) AS count
FROM workspace_bindings
WHERE ($1::BIGINT IS NULL OR application_id = $1)
  AND ($2::TEXT IS NULL OR external_user_id = $2)
  AND ($3::TEXT IS NULL OR workspace_key = $3)
  AND ($4::BOOLEAN IS NULL OR is_active = $4)
  AND ($5::TEXT IS NULL OR lifecycle_state = $5);
