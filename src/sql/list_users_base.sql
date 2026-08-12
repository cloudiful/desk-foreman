SELECT
    user_id,
    login_name,
    display_name,
    email,
    timezone,
    workspace_root,
    is_admin,
    is_active,
    must_change_password,
    last_login_at,
    created_at,
    updated_at
FROM users
WHERE deleted_at IS NULL
