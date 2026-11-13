use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "web_sessions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id: Uuid,
    pub user_id: i64,
    pub expires_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub last_seen_at: DateTimeUtc,
    pub revoked_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::UserId"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

pub fn table() -> sea_orm::sea_query::TableCreateStatement {
    super::table_for::<Entity>()
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    use sea_orm::sea_query::Index;
    vec![
        Index::create()
            .if_not_exists()
            .name("web_sessions_user_id_idx")
            .table(Entity)
            .col(Column::UserId)
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("web_sessions_expires_at_idx")
            .table(Entity)
            .col(Column::ExpiresAt)
            .to_owned(),
    ]
}
