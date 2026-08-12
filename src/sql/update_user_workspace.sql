UPDATE users
SET workspace_root = $2,
    updated_at = NOW()
WHERE user_id = $1
  AND deleted_at IS NULL
RETURNING
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
    updated_at;
