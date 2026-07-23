UPDATE mcp_tokens
SET is_active = FALSE
WHERE token_id = $1
  AND is_active = TRUE;
