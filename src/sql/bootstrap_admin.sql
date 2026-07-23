INSERT INTO users (
    login_name,
    password_hash,
    display_name,
    email,
    timezone,
    is_admin,
    is_active
)
VALUES ($1, $2, $3, $4, $5, TRUE, TRUE);
