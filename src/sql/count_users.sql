SELECT COUNT(*)::BIGINT AS count
FROM users
WHERE deleted_at IS NULL;
