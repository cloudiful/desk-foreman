UPDATE users
SET display_name = $2,
    email = $3,
    timezone = $4,
    is_admin = $5,
    is_active = $6,
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
