INSERT INTO users (
    login_name,
    password_hash,
    display_name,
    email,
    timezone,
    workspace_root,
    is_admin,
    is_active
)
VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE)
RETURNING
    user_id,
    login_name,
    display_name,
    email,
    timezone,
    workspace_root,
    is_admin,
    is_active,
    last_login_at,
    created_at,
    updated_at;
