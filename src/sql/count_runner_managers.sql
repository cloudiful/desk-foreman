SELECT COUNT(*)::BIGINT AS count
FROM runner_managers
WHERE ($1::TEXT IS NULL OR name ILIKE '%' || $1 || '%')
  AND ($2::BOOLEAN IS NULL OR enabled = $2);