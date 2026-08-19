SELECT *
FROM workspace_runners
WHERE ($1::TEXT IS NULL OR status = $1)
  AND ($2::TEXT IS NULL OR owner_kind = $2)
ORDER BY runner_id ASC
LIMIT $3 OFFSET $4;