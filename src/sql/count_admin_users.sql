SELECT COUNT(*)::BIGINT AS count
FROM users
WHERE is_admin = TRUE
  AND deleted_at IS NULL;
