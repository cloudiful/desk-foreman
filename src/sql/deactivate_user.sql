UPDATE users
SET is_active = FALSE,
    deleted_at = NOW(),
    updated_at = NOW()
WHERE user_id = $1
  AND deleted_at IS NULL;
