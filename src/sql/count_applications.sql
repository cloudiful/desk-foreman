SELECT COUNT(*)::BIGINT AS count
FROM applications
WHERE ($1::TEXT IS NULL OR name ILIKE '%' || $1 || '%')
  AND ($2::BOOLEAN IS NULL OR is_active = $2);