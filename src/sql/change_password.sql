UPDATE users
SET password_hash = $2,
    must_change_password = FALSE,
    updated_at = NOW()
WHERE user_id = $1
  AND deleted_at IS NULL;
