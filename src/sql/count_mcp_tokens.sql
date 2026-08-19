SELECT COUNT(*)::BIGINT AS count
FROM mcp_tokens
WHERE ($1::TEXT IS NULL OR token_name ILIKE '%' || $1 || '%')
  AND ($2::BIGINT IS NULL OR created_by = $2)
  AND ($3::BOOLEAN IS NULL OR is_active = $3);