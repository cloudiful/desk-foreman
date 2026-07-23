UPDATE web_sessions
SET revoked_at = NOW()
WHERE session_id = $1
  AND revoked_at IS NULL;
