SELECT
    user_id,
    login_name,
    password_hash,
    display_name,
    email,
    timezone,
    workspace_root,
    is_admin,
    is_active,
    deleted_at,
    last_login_at,
    created_at,
    updated_at
FROM users
WHERE user_id = $1
LIMIT 1;
