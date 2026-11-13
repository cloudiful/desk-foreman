use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "mcp_tokens")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub token_id: i64,
    #[sea_orm(column_type = "Text")]
    pub token_name: String,
    #[sea_orm(column_type = "Text")]
    pub token_hash: String,
    #[sea_orm(default_value = true)]
    pub is_active: bool,
    pub created_by: i64,
    pub last_used_at: Option<DateTimeUtc>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    pub expires_at: Option<DateTimeUtc>,
    #[sea_orm(
        column_type = "Array(RcOrArc::new(ColumnType::Text))",
        default_expr = "Expr::cust(\"ARRAY['workspace.read', 'workspace.search', 'workspace.shell', 'workspace.patch']::TEXT[]\")"
    )]
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    #[sea_orm(default_value = true)]
    pub network_enabled: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::CreatedBy",
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
    use sea_orm::sea_query::Index;

    let mut table = super::table_for::<Entity>();
    let mut token_hash_key = Index::create();
    token_hash_key
        .name("mcp_tokens_token_hash_key")
        .col(Column::TokenHash)
        .unique();
    table.index(&mut token_hash_key);
    table
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    use sea_orm::sea_query::Index;
    vec![
        Index::create()
            .if_not_exists()
            .name("mcp_tokens_created_by_idx")
            .table(Entity)
            .col(Column::CreatedBy)
            .to_owned(),
    ]
}
