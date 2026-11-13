use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "applications")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub application_id: i64,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(default_value = true)]
    pub is_active: bool,
    #[sea_orm(column_type = "Text")]
    pub workspace_template: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub default_shell: Option<String>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub updated_at: DateTimeUtc,
    #[sea_orm(
        column_type = "Array(RcOrArc::new(ColumnType::Text))",
        default_expr = "Expr::cust(\"ARRAY['workspace.read', 'workspace.search', 'workspace.shell', 'workspace.patch']::TEXT[]\")"
    )]
    pub default_scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    #[sea_orm(default_value = true)]
    pub network_enabled: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub fn table() -> sea_orm::sea_query::TableCreateStatement {
    use sea_orm::sea_query::Index;

    let mut table = super::table_for::<Entity>();
    let mut name_key = Index::create();
    name_key
        .name("applications_name_key")
        .col(Column::Name)
        .unique();
    table.index(&mut name_key);
    table
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    Vec::new()
}
