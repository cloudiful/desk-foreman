SELECT COUNT(*)::BIGINT AS count
FROM workspace_runners
WHERE ($1::TEXT IS NULL OR status = $1)
  AND ($2::TEXT IS NULL OR owner_kind = $2);