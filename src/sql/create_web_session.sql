INSERT INTO web_sessions (
    session_id,
    user_id,
    expires_at
)
VALUES ($1, $2, $3);
