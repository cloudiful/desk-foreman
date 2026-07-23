UPDATE application_tokens
SET is_active = FALSE
WHERE token_id = $1;
