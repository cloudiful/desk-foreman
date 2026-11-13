use anyhow::Context;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Select,
    sea_query::{Expr, ExprTrait},
};
use sqlx::PgPool;

use crate::db::native::user;
use crate::db::types::{
    CreateUserRequest, ListUsersParams, UpdateUserRequest, UserRecord, UserResponse,
};

fn native(pool: &PgPool) -> DatabaseConnection {
    DatabaseConnection::from(pool.clone())
}

fn response(model: user::Model) -> Result<UserResponse, sqlx::Error> {
    let workspace_root = model
        .workspace_root
        .ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "workspace_root".to_owned(),
            source: Box::new(sqlx::error::UnexpectedNullError),
        })?;
    Ok(UserResponse {
        user_id: model.user_id,
        login_name: model.login_name,
        display_name: model.display_name,
        email: model.email,
        timezone: model.timezone,
        workspace_root,
        is_admin: model.is_admin,
        is_active: model.is_active,
        must_change_password: model.must_change_password,
        last_login_at: model.last_login_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

pub(crate) fn record(model: user::Model) -> UserRecord {
    UserRecord {
        user_id: model.user_id,
        login_name: model.login_name,
        password_hash: model.password_hash,
        display_name: model.display_name,
        email: model.email,
        timezone: model.timezone,
        workspace_root: model.workspace_root,
        is_admin: model.is_admin,
        is_active: model.is_active,
        must_change_password: model.must_change_password,
        deleted_at: model.deleted_at,
        last_login_at: model.last_login_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn list_query(params: &ListUsersParams) -> Select<user::Entity> {
    let mut query = user::Entity::find().filter(user::Column::DeletedAt.is_null());
    if let Some(search) = params.search.as_deref() {
        let pattern = format!("%{search}%");
        query = query.filter(
            user::Column::LoginName
                .ilike(pattern.clone())
                .or(user::Column::DisplayName.ilike(pattern.clone()))
                .or(user::Column::Email.ilike(pattern)),
        );
    }
    if let Some(is_admin) = params.is_admin {
        query = query.filter(user::Column::IsAdmin.eq(is_admin));
    }
    if let Some(is_active) = params.is_active {
        query = query.filter(user::Column::IsActive.eq(is_active));
    }
    query
}

pub async fn count_admin_users(pool: &PgPool) -> anyhow::Result<i64> {
    let count = user::Entity::find()
        .filter(user::Column::IsAdmin.eq(true))
        .filter(user::Column::DeletedAt.is_null())
        .count(&native(pool))
        .await?;
    Ok(count as i64)
}

pub async fn bootstrap_admin(
    pool: &PgPool,
    login_name: &str,
    password_hash: &str,
    display_name: &str,
    email: &str,
    timezone: &str,
) -> anyhow::Result<()> {
    user::Entity::insert(user::ActiveModel {
        login_name: sea_orm::Set(login_name.to_owned()),
        password_hash: sea_orm::Set(password_hash.to_owned()),
        display_name: sea_orm::Set(display_name.to_owned()),
        email: sea_orm::Set(email.to_owned()),
        timezone: sea_orm::Set(timezone.to_owned()),
        is_admin: sea_orm::Set(true),
        is_active: sea_orm::Set(true),
        must_change_password: sea_orm::Set(true),
        ..Default::default()
    })
    .exec_without_returning(&native(pool))
    .await?;
    Ok(())
}

pub async fn find_user_by_login(
    pool: &PgPool,
    login_name: &str,
) -> anyhow::Result<Option<UserRecord>> {
    Ok(user::Entity::find()
        .filter(user::Column::LoginName.eq(login_name))
        .one(&native(pool))
        .await?
        .map(record))
}

pub async fn find_user_by_id(pool: &PgPool, user_id: i64) -> anyhow::Result<Option<UserRecord>> {
    Ok(user::Entity::find_by_id(user_id)
        .one(&native(pool))
        .await?
        .map(record))
}

pub async fn list_users(
    pool: &PgPool,
    params: &ListUsersParams,
) -> anyhow::Result<(Vec<UserResponse>, i64)> {
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = std::cmp::max(params.offset.unwrap_or(0), 0);
    let sort_by = params.sort_by.as_deref().unwrap_or("updated_at");
    let sort_dir = params.sort_dir.as_deref().unwrap_or("desc");
    let sort_column = match sort_by {
        "login_name" => user::Column::LoginName,
        "display_name" => user::Column::DisplayName,
        "created_at" => user::Column::CreatedAt,
        "last_login_at" => user::Column::LastLoginAt,
        _ => user::Column::UpdatedAt,
    };
    let sort_order = match sort_dir {
        "asc" => sea_orm::Order::Asc,
        _ => sea_orm::Order::Desc,
    };
    let db = native(pool);
    let total = list_query(params).count(&db).await? as i64;
    let rows = list_query(params)
        .order_by(sort_column, sort_order)
        .order_by(user::Column::UserId, sea_orm::Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(&db)
        .await
        .context("failed to list users")?;
    Ok((
        rows.into_iter()
            .map(response)
            .collect::<Result<Vec<_>, sqlx::Error>>()?,
        total,
    ))
}

pub async fn create_user(
    pool: &PgPool,
    request: &CreateUserRequest,
    password_hash: &str,
) -> anyhow::Result<UserResponse> {
    let model = user::Entity::insert(user::ActiveModel {
        login_name: sea_orm::Set(request.login_name.clone()),
        password_hash: sea_orm::Set(password_hash.to_owned()),
        display_name: sea_orm::Set(request.display_name.clone()),
        email: sea_orm::Set(request.email.clone()),
        timezone: sea_orm::Set(request.timezone.clone()),
        workspace_root: sea_orm::Set(request.workspace_root.clone()),
        is_admin: sea_orm::Set(request.is_admin),
        is_active: sea_orm::Set(true),
        must_change_password: sea_orm::Set(false),
        ..Default::default()
    })
    .exec_with_returning(&native(pool))
    .await?;
    Ok(response(model)?)
}

pub async fn update_user_workspace(
    pool: &PgPool,
    user_id: i64,
    workspace_root: &str,
) -> anyhow::Result<Option<UserResponse>> {
    let model = user::Entity::update_many()
        .col_expr(
            user::Column::WorkspaceRoot,
            Expr::val(workspace_root.to_owned()),
        )
        .col_expr(user::Column::UpdatedAt, Expr::cust("NOW()"))
        .filter(user::Column::UserId.eq(user_id))
        .filter(user::Column::DeletedAt.is_null())
        .exec_with_returning(&native(pool))
        .await?
        .into_iter()
        .next();
    Ok(model.map(response).transpose()?)
}

pub async fn update_user(
    pool: &PgPool,
    user_id: i64,
    request: &UpdateUserRequest,
) -> anyhow::Result<Option<UserResponse>> {
    let model = user::Entity::update_many()
        .col_expr(
            user::Column::DisplayName,
            Expr::val(request.display_name.clone()),
        )
        .col_expr(user::Column::Email, Expr::val(request.email.clone()))
        .col_expr(user::Column::Timezone, Expr::val(request.timezone.clone()))
        .col_expr(user::Column::IsAdmin, Expr::val(request.is_admin))
        .col_expr(user::Column::IsActive, Expr::val(request.is_active))
        .col_expr(user::Column::UpdatedAt, Expr::cust("NOW()"))
        .filter(user::Column::UserId.eq(user_id))
        .filter(user::Column::DeletedAt.is_null())
        .exec_with_returning(&native(pool))
        .await?
        .into_iter()
        .next();
    Ok(model.map(response).transpose()?)
}

pub async fn reset_user_password(
    pool: &PgPool,
    user_id: i64,
    password_hash: &str,
) -> anyhow::Result<bool> {
    let result = user::Entity::update_many()
        .col_expr(
            user::Column::PasswordHash,
            Expr::val(password_hash.to_owned()),
        )
        .col_expr(user::Column::MustChangePassword, Expr::val(true))
        .col_expr(user::Column::UpdatedAt, Expr::cust("NOW()"))
        .filter(user::Column::UserId.eq(user_id))
        .filter(user::Column::DeletedAt.is_null())
        .exec(&native(pool))
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn change_password(
    pool: &PgPool,
    user_id: i64,
    password_hash: &str,
) -> anyhow::Result<bool> {
    let result = user::Entity::update_many()
        .col_expr(
            user::Column::PasswordHash,
            Expr::val(password_hash.to_owned()),
        )
        .col_expr(user::Column::MustChangePassword, Expr::val(false))
        .col_expr(user::Column::UpdatedAt, Expr::cust("NOW()"))
        .filter(user::Column::UserId.eq(user_id))
        .filter(user::Column::DeletedAt.is_null())
        .exec(&native(pool))
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn deactivate_user(pool: &PgPool, user_id: i64) -> anyhow::Result<bool> {
    let result = user::Entity::update_many()
        .col_expr(user::Column::IsActive, Expr::val(false))
        .col_expr(user::Column::DeletedAt, Expr::cust("NOW()"))
        .col_expr(user::Column::UpdatedAt, Expr::cust("NOW()"))
        .filter(user::Column::UserId.eq(user_id))
        .filter(user::Column::DeletedAt.is_null())
        .exec(&native(pool))
        .await?;
    Ok(result.rows_affected > 0)
}
