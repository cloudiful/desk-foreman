SELECT COUNT(*)::BIGINT AS count
FROM users
WHERE deleted_at IS NULL
  AND ($1::TEXT IS NULL OR login_name ILIKE '%' || $1 || '%' OR display_name ILIKE '%' || $1 || '%' OR email ILIKE '%' || $1 || '%')
  AND ($2::BOOLEAN IS NULL OR is_admin = $2)
  AND ($3::BOOLEAN IS NULL OR is_active = $3);
