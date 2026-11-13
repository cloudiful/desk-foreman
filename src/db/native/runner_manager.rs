use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "runner_managers")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub runner_manager_id: i64,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub endpoint: String,
    #[sea_orm(column_type = "Text")]
    pub access_token_hash: String,
    #[sea_orm(default_value = true)]
    pub enabled: bool,
    #[sea_orm(
        column_type = "Text",
        default_value = "desk-foreman-workspace-runner:local"
    )]
    pub image: String,
    #[sea_orm(default_value = false)]
    pub network_enabled: bool,
    #[sea_orm(default_value = 262144)]
    pub max_output_bytes: i64,
    #[sea_orm(default_value = 600000)]
    pub max_timeout_ms: i64,
    #[sea_orm(default_value = 32)]
    pub max_sessions: i64,
    #[sea_orm(default_value = 256)]
    pub pids_limit: i64,
    #[sea_orm(column_type = "Text", default_value = "1g")]
    pub memory_limit: String,
    #[sea_orm(column_type = "Text", default_value = "2")]
    pub cpu_limit: String,
    #[sea_orm(column_type = "Text")]
    pub host_workspace_root: Option<String>,
    #[sea_orm(column_type = "Text", default_value = "pending")]
    pub status: String,
    pub last_seen_at: Option<DateTimeUtc>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub fn table() -> sea_orm::sea_query::TableCreateStatement {
    use sea_orm::sea_query::Index;

    let mut table = super::table_for::<Entity>();
    let mut access_token_hash_key = Index::create();
    access_token_hash_key
        .name("runner_managers_access_token_hash_key")
        .col(Column::AccessTokenHash)
        .unique();
    table.index(&mut access_token_hash_key);
    table
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    use sea_orm::sea_query::Index;
    vec![
        Index::create()
            .if_not_exists()
            .name("runner_managers_name_key")
            .table(Entity)
            .col(Column::Name)
            .unique()
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("runner_managers_enabled_status_idx")
            .table(Entity)
            .col(Column::Enabled)
            .col(Column::Status)
            .col(Column::LastSeenAt)
            .to_owned(),
    ]
}
