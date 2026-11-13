use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub audit_id: i64,
    pub actor_user_id: Option<i64>,
    pub actor_application_id: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub actor_type: String,
    #[sea_orm(column_type = "Text")]
    pub action: String,
    #[sea_orm(column_type = "Text")]
    pub target_type: String,
    #[sea_orm(column_type = "Text")]
    pub target_id: String,
    pub workspace_binding_id: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub external_user_id: Option<String>,
    #[sea_orm(
        column_type = "JsonBinary",
        default_expr = "Expr::cust(\"'{}'::JSONB\")"
    )]
    pub payload: Json,
    #[sea_orm(column_type = "Text")]
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
    pub duration_ms: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub status: Option<String>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub fn table() -> sea_orm::sea_query::TableCreateStatement {
    super::table_for::<Entity>()
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    use sea_orm::sea_query::{Index, IndexOrder};
    vec![
        Index::create()
            .if_not_exists()
            .name("audit_log_created_at_idx")
            .table(Entity)
            .col((Column::CreatedAt, IndexOrder::Desc))
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("audit_log_action_idx")
            .table(Entity)
            .col(Column::Action)
            .to_owned(),
    ]
}
