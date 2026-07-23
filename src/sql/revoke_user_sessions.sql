UPDATE web_sessions
SET revoked_at = NOW()
WHERE user_id = $1
  AND revoked_at IS NULL;
