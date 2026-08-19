SELECT COUNT(*)::BIGINT AS count
FROM application_tokens
WHERE ($1::BIGINT IS NULL OR application_id = $1)
  AND ($2::BOOLEAN IS NULL OR is_active = $2);
