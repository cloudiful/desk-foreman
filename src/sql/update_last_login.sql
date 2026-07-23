UPDATE users
SET last_login_at = NOW(),
    updated_at = NOW()
WHERE user_id = $1;
