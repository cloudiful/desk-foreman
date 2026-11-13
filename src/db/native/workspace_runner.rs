use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "workspace_runners")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub runner_id: i64,
    pub runner_manager_id: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub owner_kind: String,
    pub owner_user_id: Option<i64>,
    pub owner_workspace_binding_id: Option<i64>,
    pub owner_application_id: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub owner_principal_digest: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub container_name: String,
    #[sea_orm(column_type = "Text")]
    pub container_id: Option<String>,
    #[sea_orm(column_type = "Text", default_value = "docker")]
    pub runtime: String,
    #[sea_orm(column_type = "Text")]
    pub runtime_class: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub image_name: String,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(default_value = true)]
    pub network_enabled: bool,
    #[sea_orm(column_type = "Text")]
    pub workspace_root: String,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub last_active_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub last_observed_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub updated_at: DateTimeUtc,
    #[sea_orm(column_type = "Text")]
    pub last_error: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::runner_manager::Entity",
        from = "Column::RunnerManagerId",
        to = "super::runner_manager::Column::RunnerManagerId"
    )]
    RunnerManager,
}

impl Related<super::runner_manager::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RunnerManager.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

pub fn table() -> sea_orm::sea_query::TableCreateStatement {
    use sea_orm::sea_query::{Condition, Expr};

    let mut table = super::table_for::<Entity>();
    let owner_check = Condition::any()
        .add(
            Condition::all()
                .add(Column::OwnerKind.eq("user"))
                .add(Column::OwnerUserId.is_not_null())
                .add(Column::OwnerWorkspaceBindingId.is_null())
                .add(Column::OwnerApplicationId.is_null())
                .add(Column::OwnerPrincipalDigest.is_null()),
        )
        .add(
            Condition::all()
                .add(Column::OwnerKind.eq("workspace_binding"))
                .add(Column::OwnerUserId.is_null())
                .add(Column::OwnerWorkspaceBindingId.is_not_null())
                .add(Column::OwnerApplicationId.is_null())
                .add(Column::OwnerPrincipalDigest.is_null()),
        )
        .add(
            Condition::all()
                .add(Column::OwnerKind.eq("application_principal"))
                .add(Column::OwnerUserId.is_null())
                .add(Column::OwnerWorkspaceBindingId.is_not_null())
                .add(Column::OwnerWorkspaceBindingId.gt(0))
                .add(Column::OwnerApplicationId.is_not_null())
                .add(Column::OwnerApplicationId.gt(0))
                .add(Column::OwnerPrincipalDigest.is_not_null())
                .add(Expr::cust("owner_principal_digest ~ '^[0-9a-f]{64}$'")),
        );
    table.check(("workspace_runners_owner_check", owner_check));
    table
}

pub fn indexes() -> Vec<sea_orm::sea_query::IndexCreateStatement> {
    use sea_orm::sea_query::{ConditionalStatement, Index};
    vec![
        Index::create()
            .if_not_exists()
            .name("workspace_runners_manager_container_idx")
            .table(Entity)
            .col(Column::RunnerManagerId)
            .col(Column::ContainerName)
            .unique()
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("workspace_runners_manager_user_owner_idx")
            .table(Entity)
            .col(Column::RunnerManagerId)
            .col(Column::OwnerUserId)
            .unique()
            .and_where(Column::OwnerKind.eq("user"))
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("workspace_runners_manager_binding_owner_idx")
            .table(Entity)
            .col(Column::RunnerManagerId)
            .col(Column::OwnerWorkspaceBindingId)
            .unique()
            .and_where(Column::OwnerKind.eq("workspace_binding"))
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("workspace_runners_manager_principal_owner_idx")
            .table(Entity)
            .col(Column::RunnerManagerId)
            .col(Column::OwnerApplicationId)
            .col(Column::OwnerWorkspaceBindingId)
            .col(Column::OwnerPrincipalDigest)
            .unique()
            .and_where(Column::OwnerKind.eq("application_principal"))
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("workspace_runners_manager_status_idx")
            .table(Entity)
            .col(Column::RunnerManagerId)
            .col(Column::Status)
            .col(Column::LastObservedAt)
            .to_owned(),
        Index::create()
            .if_not_exists()
            .name("workspace_runners_status_last_active_idx")
            .table(Entity)
            .col(Column::Status)
            .col(Column::LastActiveAt)
            .to_owned(),
    ]
}
