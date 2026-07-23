UPDATE mcp_tokens
SET last_used_at = NOW()
WHERE token_hash = $1
  AND is_active = TRUE;
