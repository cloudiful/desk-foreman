use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: i64,
    #[sea_orm(column_type = "Text")]
    pub login_name: String,
    #[sea_orm(column_type = "Text")]
    pub password_hash: String,
    #[sea_orm(column_type = "Text")]
    pub display_name: String,
    #[sea_orm(column_type = "Text")]
    pub email: String,
    #[sea_orm(column_type = "Text")]
    pub timezone: String,
    #[sea_orm(column_type = "Text")]
    pub workspace_root: Option<String>,
    #[sea_orm(default_value = false)]
    pub is_admin: bool,
    #[sea_orm(default_value = true)]
    pub is_active: bool,
    #[sea_orm(default_value = false)]
    pub must_change_password: bool,
    pub deleted_at: Option<DateTimeUtc>,
    pub last_login_at: Option<DateTimeUtc>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub fn table() -> sea_orm::sea_query::TableCreateStatement {
    super::table_for::<Entity>()
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    use sea_orm::sea_query::{ConditionalStatement, Index};
    vec![
        Index::create()
            .if_not_exists()
            .name("users_login_name_active_key")
            .table(Entity)
            .col(Column::LoginName)
            .unique()
            .and_where(Column::DeletedAt.is_null())
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("users_email_active_key")
            .table(Entity)
            .col(Column::Email)
            .unique()
            .and_where(Column::DeletedAt.is_null())
            .to_owned(),
    ]
}
