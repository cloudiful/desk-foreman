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
    must_change_password,
    deleted_at,
    last_login_at,
    created_at,
    updated_at
FROM users
WHERE login_name = $1
LIMIT 1;
